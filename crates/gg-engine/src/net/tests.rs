use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read as _};
use std::mem;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;

use gg_relay::Joined;
use tracing_subscriber::layer::SubscriberExt;

#[cfg(feature = "dev")]
use gg_core::math::DVec3;

use super::handshake::{Answer, executable, preferred, resolve};
use super::link::{DIRECT, KEEP, Trip, read_line};
use super::lockstep::LOCKSTEP;
#[cfg(feature = "dev")]
use super::lockstep::UNSENT;
use super::secure::{Opened, Sealer};
use super::*;
use crate::args::{Listen, Toward};
use crate::ecs::Operation;
use crate::layer::Stack;
use crate::package::Files;
use crate::testing::Codes;
use crate::{Component, Context, Layer, Reflect, Tick};

#[derive(Clone, Component)]
pub(super) struct Score {
    pub(super) points: [u32; 2],
}

#[derive(Reflect)]
pub(super) enum Command {
    Add { player: u8, points: u32 },
}

/// Two players whose commands add to their scores. Loads the scene at `scene` and the script at
/// `script`, if given.
#[derive(Default)]
pub(super) struct Game {
    commands: Vec<Command>,
    scene: Option<PathBuf>,
    script: Option<(crate::Scripts, PathBuf)>,
    modded: bool,
    mods: Option<crate::Mods>,
    /// Whether its peers predict inputs (rollback), and what its link does to packets.
    rollback: bool,
    link: Option<Conditions>,
    networked: Rc<RefCell<Vec<(&'static str, bool)>>>,
}

impl Layer for Game {
    fn load(&mut self, cx: &mut Context) {
        if let Some((scripts, path)) = &mut self.script {
            scripts.load(cx, path).expect("loaded");
        }
        if self.modded {
            // Before the mods, which ride on it.
            cx.world_mut().register::<Score>().expect("described");
            self.mods = Some(crate::Mods::load(cx, crate::Grant::all()));
        }
    }

    fn attach(&mut self, cx: &mut Context) {
        let networked = cx.networked();
        self.networked.borrow_mut().push(("attach", networked));
        let score = cx.world_mut().spawn(Score { points: [0; 2] });
        if let Some(scene) = &self.scene {
            cx.load_scene_file(scene).expect("loaded");
        }
        if self.script.is_some() {
            cx.world_mut().add_default(score, "Spin").expect("added");
        }
    }

    fn restored(&mut self, cx: &mut Context) {
        let networked = cx.networked();
        self.networked.borrow_mut().push(("restored", networked));
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for Command::Add { player, points } in self.commands.drain(..) {
            for score in world.query_mut::<&mut Score>() {
                score.points[usize::from(player)] += points;
            }
        }
        if let Some((scripts, _)) = &mut self.script {
            scripts.run(world);
        }
        if let Some(mods) = &mut self.mods {
            mods.run(world);
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }
}

struct Peer {
    stack: Stack,
    cx: Context,
    next: u64,
}

impl Peer {
    fn new(net: &Net, cx: Context, game: Game) -> Self {
        let (rollback, link) = (game.rollback, game.link);
        let mut stack = Stack::default();
        stack.push_layer(game);
        Self::of(stack, net, cx, rollback, link)
    }

    fn of(
        mut stack: Stack,
        net: &Net,
        mut cx: Context,
        rollback: bool,
        link: Option<Conditions>,
    ) -> Self {
        let names = stack.names().to_vec();
        let mut lockstep = Lockstep::start(net, TESTED, "Tested", BUILD, &names).expect("started");
        if rollback {
            lockstep.predict();
        }
        if let Some(link) = link {
            lockstep.condition(link);
        }
        stack.lockstep(lockstep);
        stack.attach(&mut cx);
        Self { stack, cx, next: 0 }
    }

    fn host(cx: Context, game: Game, alone: u64) -> Self {
        let mut host = Self::new(&Net::Host(crate::args::Listen::at("127.0.0.1:0")), cx, game);
        for _ in 0..alone {
            host.turn();
        }
        assert_eq!(host.next, alone);
        host
    }

    fn joiner(host: &mut Self) -> Self {
        let net = host.net();
        let address = net.listening.expect("listening").to_string();
        let game = Game {
            rollback: net.window > 0,
            link: net.conditions,
            ..Game::default()
        };
        let joiner = Self::new(
            &Net::Join(crate::args::Toward::Address(address)),
            Context::seeded(2),
            game,
        );
        assert_eq!(joiner.cx.player(), Some(1));
        joiner
    }

    fn turn(&mut self) {
        self.wait();
        if self.stack.tick(&mut self.cx, Tick { number: self.next }) {
            self.next += 1;
        }
    }

    fn alone(seed: u64, game: Game, relay: &str) -> Self {
        let mut stack = Stack::default();
        let predicting = game.rollback;
        stack.push_layer(game);
        let mut cx = Context::seeded(seed);
        cx.own = crate::layer::OwnSessions {
            app: "Tested",
            build: Build::default(),
            layers: stack.names().to_vec(),
            relay: Some(relay.to_owned()),
            link: None,
            predicting,
            recorded: false,
        };
        stack.attach(&mut cx);
        Self { stack, cx, next: 0 }
    }

    fn wait(&mut self) {
        self.stack.take_asked(&mut self.cx);
        if let Some(ticks) = self.stack.net_turn(&mut self.cx, self.next) {
            self.next = ticks;
        }
        self.cx.session = self.stack.session_status();
    }

    fn phase(&self) -> Option<SessionPhase> {
        self.cx.session().map(|status| status.phase().clone())
    }

    fn net(&mut self) -> &mut Lockstep {
        self.stack.net_mut().expect("a peer")
    }

    fn set(&mut self, author: &str, player: usize, points: u32) -> Edit {
        let world = self.cx.world();
        let score = world.query::<(crate::Entity, &Score)>().next();
        let id = world.id(score.expect("a score").0).expect("an ID");
        let set = Operation::Set {
            entity: id,
            component: "Score".into(),
            path: format!("points[{player}]"),
            value: Value::UInt(u64::from(points)),
        };
        Edit::Apply {
            author: author.to_owned(),
            operations: vec![set],
        }
    }

    fn score(&self) -> [u32; 2] {
        let mut scores = self.cx.world().query::<&Score>();
        scores.next().expect("a score").points
    }
}

/// The host holds its ticks until its world is sent; the joiner starts from it at the host's next
/// tick.
fn join(host: &mut Peer, joiner: &mut Peer) {
    let at = host.next;
    let start = Instant::now();
    while host.net().from.is_none() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the joiner never connected"
        );
        host.stack.net_turn(&mut host.cx, host.next);
        joiner.turn();
        thread::sleep(Duration::from_millis(1));
    }
    until(host, joiner, |_, joiner| !joiner.net().waiting());
    assert!(joiner.next >= at, "{} {at}", joiner.next);
}

/// Turns both until each has taken its queued commands into its inputs, then to the first tick
/// neither has sent for (its delay on from its next). After it none of those applies, except ones a
/// falling delay held back, at it. One still queued might go out at a delay a busy machine raised.
fn sent(host: &mut Peer, joiner: &mut Peer) -> u64 {
    let taken = |peer: &mut Peer| {
        peer.stack
            .commands_of(0)
            .is_none_or(|queue| queue.is_empty())
    };
    until(host, joiner, |host, joiner| taken(host) && taken(joiner));
    host.net().tagged.max(joiner.net().tagged)
}

fn until(host: &mut Peer, joiner: &mut Peer, mut done: impl FnMut(&mut Peer, &mut Peer) -> bool) {
    let start = Instant::now();
    while !done(host, joiner) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the peers never got there"
        );
        host.turn();
        joiner.turn();
        thread::sleep(Duration::from_millis(1));
    }
}

/// The one ahead waits at `at`, its frames turning without ticking.
fn meet(host: &mut Peer, joiner: &mut Peer, at: u64) {
    let start = Instant::now();
    while host.next < at || joiner.next < at {
        if start.elapsed() >= Duration::from_secs(10) {
            let state = |peer: &mut Peer| {
                let next = peer.next;
                let net = peer.net();
                let (whole, acked) = (net.whole, net.acked);
                format!("at {next}, the other's inputs to {whole}, its own taken to {acked}")
            };
            panic!(
                "the peers never met at {at}: the host {}; the joiner {}",
                state(host),
                state(joiner)
            );
        }
        for peer in [&mut *host, &mut *joiner] {
            if peer.next < at {
                peer.turn();
            } else {
                peer.wait();
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// Meets at `at`, then waits until each peer has run every earlier tick on the other's inputs,
/// rolling back over mispredictions: their worlds are those both peers' inputs make.
fn settle(host: &mut Peer, joiner: &mut Peer, at: u64) {
    meet(host, joiner, at);
    let start = Instant::now();
    let settled = |peer: &mut Peer| {
        let net = peer.net();
        net.whole >= at && net.predicted.is_empty()
    };
    while !(settled(host) && settled(joiner)) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the peers never settled"
        );
        host.wait();
        joiner.wait();
        thread::sleep(Duration::from_millis(1));
    }
}

/// Turns both for `time` in real time, each frame running `each` (which may queue commands), then
/// per peer taking packets and running the ticks owed since its last frame, paced as
/// [`Stepper::step`] does. [`turn`](Peer::turn) runs one tick per turn instead.
///
/// [`Stepper::step`]: crate::main_loop::Stepper::step
fn real_time(
    host: &mut Peer,
    joiner: &mut Peer,
    time: Duration,
    each: &mut dyn FnMut(&mut Peer, &mut Peer),
) {
    let start = Instant::now();
    let mut peers = [host, joiner].map(|peer| {
        let mut stepper = crate::main_loop::Stepper::default();
        stepper.jump_to(peer.next);
        (peer, stepper, start)
    });
    while start.elapsed() < time {
        let [(host, ..), (joiner, ..)] = &mut peers;
        each(host, joiner);
        for (peer, stepper, last) in &mut peers {
            let frame = Instant::now();
            let dt = frame - mem::replace(last, frame);
            if let Some(ticks) = peer.stack.net_turn(&mut peer.cx, stepper.ticks) {
                stepper.jump_to(ticks);
            }
            let (stack, cx) = (&mut peer.stack, &mut peer.cx);
            let owed = stack.paced(dt);
            stepper.step(owed, |tick| stack.tick(cx, tick), || frame.elapsed());
            peer.next = stepper.ticks;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// Per call queues a command for each peer's player, adding to `queued` what each score comes to
/// once both have run every tick sent.
fn queue(host: &mut Peer, joiner: &mut Peer, queued: &mut [u32; 2]) {
    host.stack_commands(Command::Add {
        player: 0,
        points: 1,
    });
    joiner.stack_commands(Command::Add {
        player: 1,
        points: 10,
    });
    queued[0] += 1;
    queued[1] += 10;
}

/// A delay falls a tick at a time once the link has been fast for [`FALLS`].
#[test]
fn a_lockstep_session_s_delay_follows_its_link() {
    let _real_time = crate::testing::real_time();
    let logged = crate::testing::logged();
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    let mut queued = [0; 2];
    let mut queuing = |host: &mut Peer, joiner: &mut Peer| queue(host, joiner, &mut queued);
    real_time(
        &mut host,
        &mut joiner,
        Duration::from_millis(500),
        &mut queuing,
    );
    // 200 ms each way: more than the delay covers on a busy machine's loopback, whose frames wait
    // to turn.
    let way = Duration::from_millis(200);
    for peer in [&mut host, &mut joiner] {
        peer.net().condition(Conditions {
            latency: way,
            jitter: Duration::ZERO,
            loss: 0.0,
        });
    }
    let counts = |peer: &mut Peer| (peer.net().waits, peer.next);
    let slowed = [counts(&mut host), counts(&mut joiner)];
    real_time(&mut host, &mut joiner, Duration::from_secs(1), &mut queuing);
    let before = [counts(&mut host), counts(&mut joiner)];
    for (waited, (waits, _)) in slowed.into_iter().zip(before) {
        assert!(waits > waited.0, "none waited as the link slowed");
    }
    real_time(&mut host, &mut joiner, Duration::from_secs(2), &mut queuing);
    let mut risen = [0; 2];
    for (i, peer) in [&mut host, &mut joiner].into_iter().enumerate() {
        let ((waits, ticks), (waited, ran)) = (counts(peer), before[i]);
        let (waits, ticks) = (waits - waited, ticks - ran);
        // None idle; an eighth or so with tests on every thread, where a frame that waits long to
        // turn runs its owed ticks at once, mostly without the delay rising.
        assert!(waits * 4 < ticks, "{waits} of {ticks} ticks waited");
        risen[i] = peer.net().delay;
        assert!(
            Tick::DT * risen[i] as u32 > way && risen[i] <= MOST_DELAY,
            "{} ticks",
            risen[i]
        );
    }
    for peer in [&mut host, &mut joiner] {
        peer.net().conditions = None;
    }
    real_time(
        &mut host,
        &mut joiner,
        FALLS + Duration::from_secs(2),
        &mut queuing,
    );
    // On loopback it falls to 2; to 8 or so with tests on every thread.
    for (i, peer) in [&mut host, &mut joiner].into_iter().enumerate() {
        let delay = peer.net().delay;
        assert!(
            Tick::DT * (delay as u32) < way,
            "{delay} ticks, from {}",
            risen[i]
        );
    }
    let at = sent(&mut host, &mut joiner) + 1;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.score(), queued);
    assert_eq!(joiner.score(), queued);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let parted = ["net.desync", "net.bad_command", "net.lost"];
    assert!(
        !codes.iter().any(|code| parted.contains(&code.as_str())),
        "{codes:?}"
    );
}

/// A rollback window follows its link past [`PREDICTED`]. The one ahead slows (a host leads its
/// joiner by the time its world took to come and restore), so neither runs more than a few ticks
/// ahead, and every command applies once on both.
#[test]
fn rollback_peers_predict_as_far_as_their_link_needs_and_keep_alike_in_time() {
    let _real_time = crate::testing::real_time();
    let logged = crate::testing::logged();
    // 75 ms each way and up to 30 more, a fiftieth lost.
    let link = Conditions {
        latency: Duration::from_millis(75),
        jitter: Duration::from_millis(30),
        loss: 0.02,
    };
    let game = Game {
        rollback: true,
        link: Some(link),
        ..Game::default()
    };
    let mut host = Peer::host(Context::seeded(1), game, 1);
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    let joined = host.next as i64 - joiner.next as i64;
    let mut queued = [0; 2];
    let mut queuing = |host: &mut Peer, joiner: &mut Peer| queue(host, joiner, &mut queued);
    real_time(&mut host, &mut joiner, Duration::from_secs(3), &mut queuing);
    let mut apart = Vec::new();
    real_time(
        &mut host,
        &mut joiner,
        Duration::from_secs(1),
        &mut |host, joiner| {
            queuing(host, joiner);
            apart.push(host.next as i64 - joiner.next as i64);
        },
    );
    // Unpaced they stay 6 to 9 ticks apart; paced, 0 or 1. Both turn on this thread so a busy
    // machine slows both alike.
    let most = apart.iter().map(|apart| apart.abs()).max();
    assert!(
        most.is_some_and(|most| most <= 3),
        "{joined} apart as the joiner joined, then {apart:?}"
    );
    for peer in [&mut host, &mut joiner] {
        let window = peer.net().window;
        assert!(window > PREDICTED, "{window} ticks");
    }
    let at = sent(&mut host, &mut joiner) + 1;
    settle(&mut host, &mut joiner, at);
    assert_eq!(host.score(), queued);
    assert_eq!(joiner.score(), queued);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let parted = ["net.desync", "net.bad_command", "net.lost"];
    assert!(
        !codes.iter().any(|code| parted.contains(&code.as_str())),
        "{codes:?}"
    );
}

/// Here the host's delay was raised by its last link.
#[test]
fn a_joiner_s_inputs_go_out_from_the_tick_its_host_s_world_names() {
    let _real_time = crate::testing::real_time();
    let logged = crate::testing::logged();
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    host.net().delay = 10;
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    play(&mut host, &mut joiner, 30);
    let at = sent(&mut host, &mut joiner) + 1;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.score(), [30, 300]);
    assert_eq!(joiner.score(), [30, 300]);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let codes: Vec<String> = logged.codes.try_iter().collect();
    assert!(!codes.iter().any(|code| code == "net.desync"), "{codes:?}");
}

/// A peer whose frames stop a while (restoring a large world) doesn't make the link seem slow:
/// taken off the timing are what it took late (the way there and back is timed by the latest packet
/// alone) and what the other held while it sent nothing (its echo of the clock). So neither delay
/// goes to the most.
#[test]
fn a_peer_whose_frames_stop_awhile_leaves_the_delay_as_it_was() {
    let _real_time = crate::testing::real_time();
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    real_time(
        &mut host,
        &mut joiner,
        Duration::from_millis(200),
        &mut |_, _| {},
    );
    let stopped = Instant::now();
    while stopped.elapsed() < Duration::from_millis(500) {
        host.turn();
        thread::sleep(Duration::from_millis(1));
    }
    real_time(
        &mut host,
        &mut joiner,
        Duration::from_millis(300),
        &mut |_, _| {},
    );
    // Timed by the stop, the joiner's would be the most, 16; with tests on every thread both reach
    // 8 or so.
    let delays = [host.net().delay, joiner.net().delay];
    assert!(delays.iter().all(|&delay| delay < MOST_DELAY), "{delays:?}");
}

/// Counts each stall a peer's ticks make waiting for the other's inputs, from the first tick that
/// waited to the next that ran. Here the host's: the joiner's frames stop 300 ms, less the few
/// ticks whose inputs it had, plus the joiner's first tick's time.
#[test]
fn a_peer_counts_its_stalls_and_their_time() {
    let _real_time = crate::testing::real_time();
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    real_time(
        &mut host,
        &mut joiner,
        Duration::from_millis(200),
        &mut |_, _| {},
    );
    let counted = |peer: &mut Peer| (peer.net().stalls.count, peer.net().stalls.total);
    let before = counted(&mut host);
    let stopped = Instant::now();
    while stopped.elapsed() < Duration::from_millis(300) {
        host.turn();
        thread::sleep(Duration::from_millis(1));
    }
    real_time(
        &mut host,
        &mut joiner,
        Duration::from_millis(300),
        &mut |_, _| {},
    );
    let after = counted(&mut host);
    let stalled = after.1 - before.1;
    assert!(after.0 > before.0, "no stall counted");
    // 310 ms or so; more with tests on every thread, whose frames wait to turn.
    assert!(
        (Duration::from_millis(250)..Duration::from_millis(600)).contains(&stalled),
        "{stalled:?} in {} stalls",
        after.0 - before.0
    );
}

/// A delay or window falls to the most the link needed in the last [`FALLS`], not to what it needs
/// at a lull: a one-way time of 11 ticks, then 8 for 2 s, then 5, the 8 counting until 2 s old. A
/// need above rises at once, [`MARGIN`] deviations past the way.
#[test]
fn a_delay_falls_to_the_most_the_link_needed_in_the_last_two_seconds() {
    let _real_time = crate::testing::real_time();
    for rollback in [false, true] {
        let game = Game {
            rollback,
            ..Game::default()
        };
        let mut host = Peer::host(Context::seeded(1), game, 1);
        let mut joiner = Peer::joiner(&mut host);
        join(&mut host, &mut joiner);
        // The host's link still waits for its line when `join` returns.
        let net = joiner.net();
        // Times the way one way at `ticks`, deviating by `deviation` ticks, and follows it at `at`.
        let follow_deviating = |net: &mut Lockstep, ticks: u64, deviation: u64, at: Instant| {
            let tick = Tick::DT.as_secs_f64();
            net.link.as_mut().expect("joined").trip = Trip {
                mean: Some(2.0 * ticks as f64 * tick),
                deviation: deviation as f64 * tick,
            };
            net.follow_link(at);
            if rollback { net.window } else { net.delay }
        };
        let follow =
            |net: &mut Lockstep, ticks: u64, at: Instant| follow_deviating(net, ticks, 0, at);
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        // A delay of a tick more than the way's, a window of two more, 8 at least.
        let [most, then, least] = if rollback { [13, 10, 8] } else { [12, 9, 6] };
        assert_eq!(follow(net, 11, at(0)), most);
        for ms in (100..=2000).step_by(100) {
            assert_eq!(follow(net, 8, at(ms)), most, "at {ms} ms");
        }
        let mut has = Vec::new();
        for ms in (2010..=4000).step_by(10) {
            has.push(follow(net, 5, at(ms)));
        }
        assert!(has.ends_with(&[then; 190]), "{has:?}");
        has.clear();
        for ms in (4010..=4200).step_by(10) {
            has.push(follow(net, 5, at(ms)));
        }
        assert!(has.ends_with(&[least; 10]), "{has:?}");
        assert_eq!(follow(net, 11, at(4210)), most);
        // One way plus two deviations: 8 ticks and 2 of 2 ticks, 12, a delay of 13 and a window of
        // 14; four would be 16, the most.
        let margin = if rollback { 14 } else { 13 };
        assert_eq!(follow_deviating(net, 8, 2, at(4220)), margin);
    }
}

/// Holds each packet 20 ms, and up to 10 more, and loses a fifth.
pub(super) const SLOW: Conditions = Conditions {
    latency: Duration::from_millis(20),
    jitter: Duration::from_millis(10),
    loss: 0.2,
};

/// Rollback peers over a slow link run ahead on predicted-empty inputs and roll back to the first
/// tick whose inputs held something. Commands apply once though late, lost and out of order, and
/// worlds hash alike at each tick run on both peers' inputs.
#[test]
fn rollback_peers_over_a_slow_link_apply_each_command_once_and_hash_alike() {
    let _real_time = crate::testing::real_time();
    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    let game = Game {
        rollback: true,
        link: Some(SLOW),
        ..Game::default()
    };
    let mut host = Peer::host(Context::seeded(1), game, 1);
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    // The host's player at each turn, the joiner's at every third, as keys change.
    let mut queued = [0; 2];
    for turn in 0..300 {
        host.stack_commands(Command::Add {
            player: 0,
            points: 1,
        });
        queued[0] += 1;
        if turn % 3 == 0 {
            joiner.stack_commands(Command::Add {
                player: 1,
                points: 10,
            });
            queued[1] += 10;
        }
        host.turn();
        joiner.turn();
        thread::sleep(Duration::from_millis(1));
    }
    // Past the ticks the last commands went out for.
    let at = host.next.max(joiner.next) + ROLLBACK_DELAY + 1;
    settle(&mut host, &mut joiner, at);
    assert_eq!(host.score(), queued);
    assert_eq!(joiner.score(), queued);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let rolled = |peer: &mut Peer| peer.net().rolled;
    let ([rollbacks, again, most], [theirs, ..]) = (rolled(&mut host), rolled(&mut joiner));
    assert!(rollbacks > 0 && theirs > 0, "{rollbacks} {theirs}");
    assert!(
        again >= rollbacks && most <= MOST_PREDICTED,
        "{again} {most}"
    );
    let codes: Vec<String> = logged.try_iter().collect();
    let parted = ["net.desync", "net.bad_command", "net.lost"];
    assert!(
        !codes.iter().any(|code| parted.contains(&code.as_str())),
        "{codes:?}"
    );
}

/// [`Editor`]'s commands, which edit.
#[derive(Default)]
struct Edits(Vec<Command>);

impl Commands for Edits {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn value(&self, i: usize) -> Value {
        self.0[i].to_value()
    }

    fn push(&mut self, value: &Value) -> Result<()> {
        self.0.push(Command::from_value(value)?);
        Ok(())
    }

    fn edits(&self) -> bool {
        true
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

/// Adds its commands' points to the score it attaches, and counts them as applied, as the
/// Inspector's edit log does: a rerun tick would count them again.
struct Editor {
    queued: Edits,
    applied: Rc<Cell<u32>>,
}

impl Layer for Editor {
    fn attach(&mut self, cx: &mut Context) {
        cx.world_mut().spawn(Score { points: [0; 2] });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for Command::Add { player, points } in self.queued.0.drain(..) {
            for score in world.query_mut::<&mut Score>() {
                score.points[usize::from(player)] += points;
            }
            self.applied.set(self.applied.get() + 1);
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.queued)
    }
}

/// In a rollback session a tick applying an editing queue's command (as the Inspector's) runs only
/// once it has both peers' inputs: the peer whose it is waits, the other rolls back to it. So each
/// applies once per peer, as a layer that counts what it did needs.
#[test]
fn rollback_peers_apply_each_edit_once() {
    let _real_time = crate::testing::real_time();
    let peer = |net: &Net, cx: Context| {
        let applied = Rc::new(Cell::new(0));
        let mut stack = Stack::default();
        stack.push_layer(Editor {
            queued: Edits::default(),
            applied: Rc::clone(&applied),
        });
        (Peer::of(stack, net, cx, true, Some(SLOW)), applied)
    };
    let (mut host, by_host) = peer(
        &Net::Host(crate::args::Listen::at("127.0.0.1:0")),
        Context::seeded(1),
    );
    host.turn();
    let address = host.net().listening.expect("listening").to_string();
    let (mut joiner, by_joiner) = peer(
        &Net::Join(crate::args::Toward::Address(address)),
        Context::seeded(2),
    );
    join(&mut host, &mut joiner);
    let edit = |peer: &mut Peer, player: u8| {
        let queue = peer
            .stack
            .commands_of(0)
            .expect("the editor queues commands");
        let command = Command::Add { player, points: 1 };
        queue.push(&command.to_value()).expect("queued");
    };
    let mut queued = [0; 2];
    for turn in 0..200 {
        if turn % 7 == 0 {
            edit(&mut host, 0);
            queued[0] += 1;
        }
        if turn % 5 == 0 {
            edit(&mut joiner, 1);
            queued[1] += 1;
        }
        host.turn();
        joiner.turn();
        thread::sleep(Duration::from_millis(1));
    }
    let at = host.next.max(joiner.next) + ROLLBACK_DELAY + 1;
    settle(&mut host, &mut joiner, at);
    let all = queued[0] + queued[1];
    assert_eq!((by_host.get(), by_joiner.get()), (all, all));
    assert_eq!(host.score(), queued);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
}

/// [`Stager`]'s command: asks for the scene at this path from the asset root.
#[derive(Reflect)]
struct Stage(String);

/// Loads `scenes/menu.ron` on attach; each tick asks for the scenes its commands name.
#[derive(Default)]
struct Stager(Vec<Stage>);

impl Layer for Stager {
    fn attach(&mut self, cx: &mut Context) {
        let root = cx.root.clone().expect("a root");
        cx.load_scene_file(root.join("scenes/menu.ron"))
            .expect("loaded");
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for Stage(scene) in self.0.drain(..) {
            world.change_scene(&scene);
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.0)
    }
}

/// Lockstep peers (rolling back if `rollback`) each go to a scene the other's command asked for at
/// the same tick, load the texture it names on both, and hash alike, without a desync.
fn peers_change_scenes_in_step(rollback: bool) {
    use std::fs;

    use crate::dirs::tests::Scratch;

    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    let dir = Scratch::new(&format!("net-scene-change-{rollback}"));
    let root = dir.0.clone();
    fs::create_dir_all(root.join("scenes")).expect("a directory");
    let png =
        crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    fs::write(root.join("moss.png"), png).expect("a PNG");
    let sprite = |id: u64, texture: &str| {
        format!(
            "        \"{id:016x}\": (\n            Sprite: (\n                texture: \
                 {texture},\n            ),\n        ),\n"
        )
    };
    let scene = |entities: &str| format!("(\n    entities: {{\n{entities}    }},\n)\n");
    let menu = scene(&sprite(1, "None"));
    let level = scene(&(sprite(2, "Some(\"moss.png\")") + &sprite(3, "None")));
    fs::write(root.join("scenes/menu.ron"), menu).expect("the menu");
    fs::write(root.join("scenes/level.ron"), level).expect("the level");
    let link = rollback.then_some(SLOW);
    let peer = |net: &Net, seed: u64| {
        let mut stack = Stack::default();
        stack.push_layer(Stager::default());
        Peer::of(
            stack,
            net,
            Context::seeded(seed).root(&root),
            rollback,
            link,
        )
    };
    let mut host = peer(&Net::Host(crate::args::Listen::at("127.0.0.1:0")), 1);
    host.turn();
    let address = host.net().listening.expect("listening").to_string();
    let mut joiner = peer(&Net::Join(crate::args::Toward::Address(address)), 2);
    join(&mut host, &mut joiner);
    let stage = |peer: &mut Peer, scene: &str| {
        let queue = peer
            .stack
            .commands_of(0)
            .expect("the stager queues commands");
        let command = Stage(scene.to_owned()).to_value();
        queue.push(&command).expect("queued");
    };
    let ids = |peer: &Peer| {
        let world = peer.cx.world();
        let mut ids: Vec<u64> = (world.query::<crate::Entity>())
            .map(|entity| world.id(entity).expect("alive").get())
            .collect();
        ids.sort_unstable();
        ids
    };
    for (by_joiner, scene, entities) in [
        (true, "scenes/level.ron", vec![2, 3]),
        (false, "scenes/menu.ron", vec![1]),
        (false, "scenes/level.ron", vec![2, 3]),
    ] {
        stage(if by_joiner { &mut joiner } else { &mut host }, scene);
        for _ in 0..20 {
            host.turn();
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
        }
        let at = sent(&mut host, &mut joiner) + 1;
        settle(&mut host, &mut joiner, at);
        assert_eq!((ids(&host), ids(&joiner)), (entities.clone(), entities));
        assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    }
    for peer in [&host, &joiner] {
        assert!(peer.cx.world().assets().texture("moss.png").is_some());
    }
    // Each takes the last as final as later ticks run; a rollback peer may have taken it as
    // predicted.
    let at = host.next.max(joiner.next) + 2;
    settle(&mut host, &mut joiner, at);
    let codes: Vec<String> = logged.try_iter().collect();
    let replaced = (codes.iter())
        .filter(|code| *code == "scene.replaced")
        .count();
    assert_eq!(replaced, 6, "three on each peer: {codes:?}");
    let parted = [
        "net.desync",
        "net.bad_command",
        "net.lost",
        "asset.stand_in",
    ];
    assert!(
        !codes.iter().any(|code| parted.contains(&code.as_str())),
        "{codes:?}"
    );
}

/// Whether [`Vetoer`] vetoes scene changes: a resource, so rollback takes it back with the world.
#[derive(Clone)]
struct Vetoed;

/// [`Vetoer`]'s command.
#[derive(Reflect)]
enum Stagehand {
    /// Vetoes every scene change from then on.
    Veto,
    /// Asks for the scene at this path from the asset root, unless vetoed.
    Go(String),
}

/// Loads `scenes/menu.ron` on attach and applies its commands each tick.
#[derive(Default)]
struct Vetoer(Vec<Stagehand>);

impl Layer for Vetoer {
    fn attach(&mut self, cx: &mut Context) {
        let root = cx.root.clone().expect("a root");
        cx.load_scene_file(root.join("scenes/menu.ron"))
            .expect("loaded");
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for command in self.0.drain(..) {
            match command {
                Stagehand::Veto => {
                    world.insert_resource(Vetoed);
                }
                Stagehand::Go(scene) if world.resource::<Vetoed>().is_none() => {
                    world.change_scene(&scene);
                }
                Stagehand::Go(_) => {}
            }
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.0)
    }
}

/// A rollback peer that changed the scene at a tick run on the other's predicted inputs, whose
/// actual ones held a veto, takes it back on rolling back: the world, its scene file and the logs
/// keep nothing of it, both hash alike, and no peer logs `scene.replaced` for it.
#[test]
fn a_scene_change_rollback_takes_back_leaves_nothing_of_it() {
    use std::fs;

    use crate::dirs::tests::Scratch;

    let _real_time = crate::testing::real_time();
    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    let dir = Scratch::new("net-scene-change-vetoed");
    let root = dir.0.clone();
    fs::create_dir_all(root.join("scenes")).expect("a directory");
    let scene = |id: u64| {
        format!(
            "(\n    entities: {{\n        \"{id:016x}\": (\n            Sprite: (),\n        \
                 ),\n    }},\n)\n"
        )
    };
    fs::write(root.join("scenes/menu.ron"), scene(1)).expect("the menu");
    fs::write(root.join("scenes/level.ron"), scene(2)).expect("the level");
    // Slower than the ticks peers predict ahead, so the joiner runs the tick of its change before
    // the host's veto of it comes.
    let link = Conditions {
        latency: Duration::from_millis(80),
        jitter: Duration::ZERO,
        loss: 0.0,
    };
    let peer = |net: &Net, seed: u64| {
        let mut stack = Stack::default();
        stack.push_layer(Vetoer::default());
        Peer::of(
            stack,
            net,
            Context::seeded(seed).root(&root),
            true,
            Some(link),
        )
    };
    let mut host = peer(&Net::Host(crate::args::Listen::at("127.0.0.1:0")), 1);
    host.turn();
    let address = host.net().listening.expect("listening").to_string();
    let mut joiner = peer(&Net::Join(crate::args::Toward::Address(address)), 2);
    join(&mut host, &mut joiner);
    let at = host.next.max(joiner.next) + 1;
    meet(&mut host, &mut joiner, at);
    let queue = |peer: &mut Peer, command: Stagehand| {
        let queue = peer
            .stack
            .commands_of(0)
            .expect("the vetoer queues commands");
        queue.push(&command.to_value()).expect("queued");
    };
    // For the same tick, the host's first.
    queue(&mut host, Stagehand::Veto);
    queue(&mut joiner, Stagehand::Go("scenes/level.ron".to_owned()));
    for _ in 0..40 {
        host.turn();
        joiner.turn();
        thread::sleep(Duration::from_millis(1));
    }
    let settled = host.next.max(joiner.next) + ROLLBACK_DELAY + 1;
    settle(&mut host, &mut joiner, settled);
    for peer in [&host, &joiner] {
        let world = peer.cx.world();
        assert_eq!(world.scene(), None);
        assert!(world.resource::<Vetoed>().is_some());
        let files: Vec<&str> = (peer.cx.scenes.files.iter())
            .map(|file| file.author.as_str())
            .collect();
        assert_eq!(files, ["scenes/menu.ron"]);
    }
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let [rollbacks, ..] = joiner.net().rolled;
    assert!(rollbacks > 0, "the joiner rolled back");
    let codes: Vec<String> = logged.try_iter().collect();
    assert!(
        codes.iter().any(|code| code == "scene.entered"),
        "the joiner's prediction changed the scene: {codes:?}"
    );
    let parted = ["scene.replaced", "net.desync", "net.lost"];
    assert!(
        !codes.iter().any(|code| parted.contains(&code.as_str())),
        "{codes:?}"
    );
}

/// [`peers_change_scenes_in_step`] in lockstep.
#[test]
fn lockstep_peers_change_scenes_in_step() {
    let _real_time = crate::testing::real_time();
    peers_change_scenes_in_step(false);
}

/// [`peers_change_scenes_in_step`] rolling back over a slow link.
#[test]
fn rollback_peers_change_scenes_in_step() {
    let _real_time = crate::testing::real_time();
    peers_change_scenes_in_step(true);
}

/// A peer in asset root `root` of a [`Walker`](crate::cells::tests::Walker) world of cells, rolling
/// back over a slow link if `rollback`.
fn walker(net: &Net, root: &std::path::Path, seed: u64, rollback: bool) -> Peer {
    let mut stack = Stack::default();
    stack.push_layer(crate::cells::tests::Walker::default());
    let cx = Context::seeded(seed).root(root);
    Peer::of(stack, net, cx, rollback, rollback.then_some(SLOW))
}

fn walk(peer: &mut Peer, walk: crate::cells::tests::Walk) {
    let queue = peer
        .stack
        .commands_of(0)
        .expect("the walker queues commands");
    queue.push(&walk.to_value()).expect("queued");
}

/// Lockstep peers (rolling back if `rollback`) bring in and take out the same cells as each moves
/// the viewer or an entity in turn, cell 1 coming back from what it keeps. Worlds hash alike once
/// both have run each tick on both peers' inputs, though rolling back reruns ticks across entries
/// and exits.
fn peers_stream_cells_in_step(rollback: bool) {
    use crate::cells::tests::{Walk, id_of, project, xs};
    use crate::dirs::tests::Scratch;

    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    let dir = Scratch::new(&format!("net-cells-{rollback}"));
    let root = project(&dir.0);
    let mut host = walker(
        &Net::Host(crate::args::Listen::at("127.0.0.1:0")),
        &root,
        1,
        rollback,
    );
    host.turn();
    let address = host.net().listening.expect("listening").to_string();
    let mut joiner = walker(
        &Net::Join(crate::args::Toward::Address(address)),
        &root,
        2,
        rollback,
    );
    join(&mut host, &mut joiner);
    for (by_joiner, step, cells) in [
        (true, Walk::Viewer(25.0), vec![0, 1, 2, 3]),
        (false, Walk::Crate(12.0), vec![0, 1, 2, 3]),
        (true, Walk::Viewer(100.0), vec![]),
        (false, Walk::Viewer(5.0), vec![0, 1]),
    ] {
        walk(if by_joiner { &mut joiner } else { &mut host }, step);
        for _ in 0..20 {
            host.turn();
            joiner.turn();
            thread::sleep(Duration::from_millis(1));
        }
        let at = sent(&mut host, &mut joiner) + 1;
        settle(&mut host, &mut joiner, at);
        let (ours, theirs) = (host.cx.world(), joiner.cx.world());
        assert_eq!((xs(ours), xs(theirs)), (cells.clone(), cells), "{step:?}");
        assert_eq!(ours.hash(), theirs.hash(), "{step:?}");
    }
    for peer in [&host, &joiner] {
        let world = peer.cx.world();
        let entity = world.entity(id_of(1)).expect("in");
        let at = world.get::<crate::Transform>(entity).expect("placed");
        assert_eq!(at.translation.x, 12.0, "as it was left");
    }
    if rollback {
        for peer in [&mut host, &mut joiner] {
            let [rollbacks, ..] = peer.net().rolled;
            assert!(rollbacks > 0, "rolled back");
        }
    }
    let codes: Vec<String> = logged.try_iter().collect();
    let parted = ["net.desync", "net.bad_command", "net.lost", "cells.refused"];
    assert!(
        !codes.iter().any(|code| parted.contains(&code.as_str())),
        "{codes:?}"
    );
}

/// [`peers_stream_cells_in_step`] in lockstep.
#[test]
fn lockstep_peers_stream_cells_in_step() {
    let _real_time = crate::testing::real_time();
    peers_stream_cells_in_step(false);
}

/// [`peers_stream_cells_in_step`] rolling back over a slow link.
#[test]
fn rollback_peers_stream_cells_in_step() {
    let _real_time = crate::testing::real_time();
    peers_stream_cells_in_step(true);
}

/// A joiner whose cell file isn't its host's, so their worlds part as the cell comes in, names it
/// and leaves to retry later, not taking the host's world and parting again at the next cell.
#[test]
fn a_joiner_whose_cell_file_differs_names_it_and_leaves() {
    use crate::cells::tests::{Walk, project};
    use crate::dirs::tests::Scratch;

    let _real_time = crate::testing::real_time();
    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    let dirs = ["net-cell-file-host", "net-cell-file-joiner"].map(Scratch::new);
    let [host_root, joiner_root] = [&dirs[0], &dirs[1]].map(|dir| project(&dir.0));
    let two = joiner_root.join("cells/2_0_0.ron");
    let other = std::fs::read_to_string(&two).expect("a cell");
    std::fs::write(&two, other.replace("25.0", "26.0")).expect("written");
    let mut host = walker(
        &Net::Host(crate::args::Listen::at("127.0.0.1:0")),
        &host_root,
        1,
        false,
    );
    host.turn();
    let address = host.net().listening.expect("listening").to_string();
    let mut joiner = walker(
        &Net::Join(crate::args::Toward::Address(address)),
        &joiner_root,
        2,
        false,
    );
    join(&mut host, &mut joiner);
    walk(&mut joiner, Walk::Viewer(25.0));
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    while !seen.iter().any(|code| code == "net.cell_file") {
        assert!(started.elapsed() < Duration::from_secs(10), "{seen:?}");
        host.turn();
        joiner.turn();
        thread::sleep(Duration::from_millis(1));
        seen.extend(logged.try_iter());
    }
    assert!(seen.iter().any(|code| code == "net.desync"), "{seen:?}");
    assert!(seen.iter().any(|code| code == "net.lost"), "{seen:?}");
}

/// A joiner joining after its host's cells came and went starts from the host's world, with what
/// the cells keep, and brings them in again alike: cell 1's entity where the host left it.
#[test]
fn a_joiner_takes_what_its_host_s_cells_keep() {
    use crate::cells::tests::{Walk, id_of, project, xs};
    use crate::dirs::tests::Scratch;

    let _real_time = crate::testing::real_time();
    let dir = Scratch::new("net-cells-joiner");
    let root = project(&dir.0);
    let mut host = walker(
        &Net::Host(crate::args::Listen::at("127.0.0.1:0")),
        &root,
        1,
        false,
    );
    for step in [Walk::Viewer(25.0), Walk::Crate(12.0), Walk::Viewer(100.0)] {
        walk(&mut host, step);
        // Past the tick its commands go out for, `DELAY` on, as a lone peer's do.
        for _ in 0..=DELAY {
            host.turn();
        }
    }
    let kept = |peer: &Peer| {
        let world = peer.cx.world();
        world.resource::<crate::cells::CellState>().cloned()
    };
    let held = kept(&host).expect("the host's cells came in");
    assert_eq!((held.cells_in.len(), held.kept.len()), (0, 1));
    let address = host.net().listening.expect("listening").to_string();
    let mut joiner = walker(
        &Net::Join(crate::args::Toward::Address(address)),
        &root,
        2,
        false,
    );
    join(&mut host, &mut joiner);
    assert_eq!(kept(&joiner), Some(held), "the host's");
    walk(&mut joiner, Walk::Viewer(5.0));
    for _ in 0..20 {
        host.turn();
        joiner.turn();
        thread::sleep(Duration::from_millis(1));
    }
    let at = sent(&mut host, &mut joiner) + 1;
    settle(&mut host, &mut joiner, at);
    for peer in [&host, &joiner] {
        let world = peer.cx.world();
        assert_eq!(xs(world), [0, 1]);
        let entity = world.entity(id_of(1)).expect("in");
        let at = world.get::<crate::Transform>(entity).expect("placed");
        assert_eq!(at.translation.x, 12.0, "where the host left it");
    }
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
}

fn play(host: &mut Peer, joiner: &mut Peer, turns: usize) {
    for _ in 0..turns {
        host.stack_commands(Command::Add {
            player: 0,
            points: 1,
        });
        joiner.stack_commands(Command::Add {
            player: 1,
            points: 10,
        });
        host.turn();
        joiner.turn();
        thread::sleep(Duration::from_millis(1));
    }
}

/// The fields of each `net.desync_field` event, by name, as text.
struct Partings(mpsc::Sender<BTreeMap<String, String>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Partings {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        struct Fields(BTreeMap<String, String>);
        impl tracing::field::Visit for Fields {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                self.0.insert(field.name().to_owned(), value.to_owned());
            }
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.insert(field.name().to_owned(), format!("{value:?}"));
            }
        }
        let mut fields = Fields(BTreeMap::new());
        event.record(&mut fields);
        if fields
            .0
            .get("code")
            .is_some_and(|code| code == "net.desync_field")
        {
            let _ = self.0.send(fields.0);
        }
    }
}

/// A joiner joins within [`DELAY`] ticks of the host's start (it has sent no inputs before) and
/// starts from its world; each tick runs on both players' commands on both peers, which hash alike.
/// Worlds that part are named at their tick once by each peer, the joiner naming the field they
/// parted at with both values, and made the same again once.
#[test]
fn two_peers_run_each_tick_on_both_players_inputs() {
    let _real_time = crate::testing::real_time();
    let (codes, logged) = mpsc::channel();
    let (partings, parted) = mpsc::channel();
    let subscriber = tracing_subscriber::registry()
        .with(Codes(codes))
        .with(Partings(partings));
    let _logging = tracing::subscriber::set_default(subscriber);
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    play(&mut host, &mut joiner, 50);
    let at = sent(&mut host, &mut joiner) + 1;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.score(), joiner.score());
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let [ours, theirs] = host.score();
    assert!(ours > 0 && theirs > 0, "{ours} {theirs}");

    // The host's world changed behind the session's back between ticks: the first tick after parts
    // the worlds by player 1's points.
    play(&mut host, &mut joiner, 10);
    let (edited, points) = (host.next, host.score()[1]);
    let edit = host.set("cheat", 1, 5);
    host.cx.apply_edit(edit).expect("applied");
    // The host runs ahead as far as its joiner's inputs reach, the joiner past it as far as the
    // host's do: the joiner runs ticks after the parting one before the host's world after it
    // comes, as over a link slower than loopback.
    for _ in 0..DELAY * 2 {
        host.stack_commands(Command::Add {
            player: 0,
            points: 1,
        });
        host.turn();
        thread::sleep(Duration::from_millis(1));
    }
    for _ in 0..DELAY * 4 {
        joiner.turn();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(joiner.next > edited + 1, "{} {edited}", joiner.next);
    play(&mut host, &mut joiner, 100);
    let start = Instant::now();
    loop {
        let at = host.next.max(joiner.next) + 1;
        meet(&mut host, &mut joiner, at);
        if host.cx.world().hash() == joiner.cx.world().hash() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the peers never came back together"
        );
    }
    assert_eq!(host.score(), joiner.score());
    let desyncs = logged.try_iter().filter(|code| code == "net.desync");
    assert_eq!(desyncs.count(), 2);
    let partings: Vec<BTreeMap<String, String>> = parted.try_iter().collect();
    let [parting] = &partings[..] else {
        panic!("the joiner alone names it, once: {partings:?}");
    };
    let world = host.cx.world();
    let score = world.query::<(crate::Entity, &Score)>().next();
    let score = world.id(score.expect("a score").0).expect("an ID");
    assert_eq!(parting["tick"], edited.to_string(), "{parting:?}");
    assert_eq!(parting["entity"], score.to_string(), "{parting:?}");
    assert_eq!(parting["field"], "Score.points[1]", "{parting:?}");
    let value = |peer: &str| {
        let value = parting[peer].strip_prefix("UInt(");
        let value = value.and_then(|value| value.strip_suffix(')'));
        value.and_then(|value| value.parse::<i64>().ok())
    };
    let (Some(joiner), Some(host)) = (value("joiner"), value("host")) else {
        panic!("two points: {parting:?}");
    };
    assert_eq!(joiner - host, i64::from(points) - 5, "{parting:?}");
}

/// Hosts and joiners are networked as their layers attach, a joiner as they pick up from its host's
/// world, and so is a host restarted from a snapshot (as `gg run` does). So a game keeps what only
/// a run alone has (a player it plays itself) out of sessions' worlds.
#[test]
fn peers_are_networked_as_their_layers_attach_and_pick_up() {
    let _real_time = crate::testing::real_time();
    let seen = |game: &Game| Rc::clone(&game.networked);
    let game = Game::default();
    let host_saw = seen(&game);
    let mut host = Peer::host(Context::seeded(1), game, 1);
    let address = host.net().listening.expect("listening").to_string();
    let game = Game::default();
    let joiner_saw = seen(&game);
    let mut joiner = Peer::new(
        &Net::Join(crate::args::Toward::Address(address)),
        Context::seeded(2),
        game,
    );
    join(&mut host, &mut joiner);
    assert_eq!(*host_saw.borrow(), [("attach", true)]);
    assert_eq!(*joiner_saw.borrow(), [("attach", true), ("restored", true)]);

    let of = crate::snapshot::Of {
        world: host.cx.world(),
        scenes: &host.cx.scenes.files,
        cached: &host.cx.scenes.cached,
        open: &host.cx.scenes.open,
        scripts: &[],
        mods: &[],
        seen: None,
    };
    let playing = &crate::snapshot::Run::Playing;
    let text = crate::snapshot::write(&of, playing, "Tested", host.stack.names(), host.next);
    let from = "the host's world".to_owned();
    let snapshot = crate::snapshot::Snapshot::parse(text.expect("written"), from, "Tested");
    let game = Game::default();
    let restarted_saw = seen(&game);
    let mut stack = Stack::default();
    stack.push_layer(game);
    let names = stack.names().to_vec();
    let net = Net::Host(crate::args::Listen::at("127.0.0.1:0"));
    stack.lockstep(Lockstep::start(&net, TESTED, "Tested", BUILD, &names).expect("started"));
    let mut cx = Context::seeded(3);
    cx.world_mut().register::<Score>().expect("described");
    let restored = stack.restore(&mut cx, &snapshot.expect("read"), false);
    restored.expect("restored");
    assert!(cx.networked());
    assert_eq!(*restarted_saw.borrow(), [("restored", true)]);
}

/// A joiner its host refuses (another app) closes its connection and retries only after
/// [`REFUSED`], so the host refuses it once.
#[test]
fn a_refused_joiner_waits_before_trying_again() {
    let _real_time = crate::testing::real_time();
    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let address = host.net().listening.expect("listening").to_string();
    let join = Net::Join(crate::args::Toward::Address(address));
    let mut other = Lockstep::start(&join, TESTED, "Other", BUILD, &["Game"]).expect("started");
    let mut refused = 0;
    until_refused_and_after(|| {
        host.turn();
        assert!(matches!(other.turn(0), Turn::None));
        refused += logged
            .try_iter()
            .filter(|code| code == "net.refused")
            .count();
        refused
    });
    assert_eq!(refused, 2);
    assert!(other.link.is_none() && host.net().link.is_none());
}

/// A joiner given another code than its host's is refused in its handshake by the host and by
/// itself, which retries only after [`REFUSED`], never taking the joiner's place; a joiner given
/// the host's code then joins.
#[test]
fn a_joiner_given_another_code_is_refused() {
    let _real_time = crate::testing::real_time();
    let logged = crate::testing::logged();
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let address = host.net().listening.expect("listening").to_string();
    // Of the host's app and layers, so its code alone differs.
    let layers = host.net().layers.clone();
    let layers: Vec<&str> = layers.split(' ').collect();
    let code = "0000-0000-0000-0000".parse().expect("a code");
    let net = Net::Join(crate::args::Toward::Address(address));
    let mut other = Lockstep::start(&net, code, "Tested", BUILD, &layers).expect("started");
    let mut refused = 0;
    until_refused_and_after(|| {
        host.turn();
        assert!(matches!(other.turn(0), Turn::None));
        assert!(other.link.is_none() && host.net().link.is_none());
        let codes = logged.codes.try_iter();
        refused += codes.filter(|code| code == "net.refused").count();
        refused
    });
    assert_eq!(refused, 2);
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
}

/// Turns a refused joiner and its host until each side has logged a refusal (`turn` counts them),
/// then a fifth of a second, in which a joiner retrying sooner than [`REFUSED`] would be refused
/// again.
fn until_refused_and_after(mut turn: impl FnMut() -> usize) {
    let start = Instant::now();
    let mut both = None;
    while both.is_none_or(|both: Instant| both.elapsed() < Duration::from_millis(200)) {
        let refused = turn();
        assert!(start.elapsed() < REFUSED, "{refused} refusals");
        if refused >= 2 {
            both.get_or_insert_with(Instant::now);
        }
        thread::sleep(Duration::from_millis(1));
    }
}

thread_local! {
    /// Whether this thread's test links ignore the other end's `at`, so their packets stay on the
    /// relay, as behind routers hole punching doesn't get through.
    pub(super) static UNPUNCHED: Cell<bool> = const { Cell::new(false) };
}

struct Unpunched;

impl Unpunched {
    fn new() -> Self {
        UNPUNCHED.set(true);
        Self
    }
}

impl Drop for Unpunched {
    fn drop(&mut self) {
        UNPUNCHED.set(false);
    }
}

fn relay() -> (gg_relay::Relay, String) {
    let relay = gg_relay::serve("127.0.0.1:0", gg_relay::Limits::default()).expect("a relay");
    let at = relay.address().to_string();
    (relay, at)
}

fn relayed(at: &str, game: impl Fn() -> Game) -> (Peer, Peer) {
    let listen = Listen {
        address: None,
        relay: Some(at.to_owned()),
    };
    let mut host = Peer::new(&Net::Host(listen), Context::seeded(1), game());
    assert_eq!(host.net().listening, None);
    let toward = Toward::Relay(at.to_owned());
    let joiner = Peer::new(&Net::Join(toward), Context::seeded(2), game());
    (host, joiner)
}

/// Plays both until each has logged `net.direct`; fails after 10 s.
fn until_direct(host: &mut Peer, joiner: &mut Peer, logged: &crate::testing::Logged) {
    let start = Instant::now();
    let mut direct = 0;
    while direct < 2 {
        assert!(start.elapsed() < Duration::from_secs(10), "never direct");
        play(host, joiner, 1);
        direct += logged
            .codes
            .try_iter()
            .filter(|code| code == "net.direct")
            .count();
    }
    assert_eq!(direct, 2);
}

/// A host registered alone at a relay and a joiner given its address and the code meet there by the
/// code's room and play in step, worlds hashing alike. Their stream goes through the relay; packets
/// go direct once both see direct works, the relay forwarding none after. With `unpunched` (as
/// behind routers hole punching doesn't get through), all goes through it.
fn peers_meet_through_a_relay(rollback: bool, unpunched: bool) {
    let logged = crate::testing::logged();
    let _unpunched = unpunched.then(Unpunched::new);
    let (relay, at) = relay();
    let (mut host, mut joiner) = relayed(&at, || Game {
        rollback,
        ..Game::default()
    });
    join(&mut host, &mut joiner);
    play(&mut host, &mut joiner, 50);
    if !unpunched {
        until_direct(&mut host, &mut joiner, &logged);
        play(&mut host, &mut joiner, 10);
    }
    let forwarded = relay.forwarded();
    play(&mut host, &mut joiner, 50);
    let at = sent(&mut host, &mut joiner) + 1;
    settle(&mut host, &mut joiner, at);
    assert_eq!(host.score(), joiner.score());
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let [ours, theirs] = host.score();
    assert!(ours > 0 && theirs > 0, "{ours} {theirs}");
    for peer in [&mut host, &mut joiner] {
        let link = peer.net().link.as_ref().expect("a link");
        assert!(link.relayed.is_some() && link.reflexive.is_some());
    }
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let count = |code: &str| codes.iter().filter(|logged| *logged == code).count();
    if unpunched {
        assert!(relay.forwarded() > forwarded);
        assert_eq!(count("net.direct"), 0, "{codes:?}");
    } else {
        assert_eq!(relay.forwarded(), forwarded);
        assert_eq!(count("net.relayed"), 0, "{codes:?}");
    }
    relay.stop();
}

#[test]
fn lockstep_peers_meet_through_a_relay() {
    let _real_time = crate::testing::real_time();
    peers_meet_through_a_relay(false, false);
}

#[test]
fn rollback_peers_meet_through_a_relay() {
    let _real_time = crate::testing::real_time();
    peers_meet_through_a_relay(true, false);
}

#[test]
fn peers_whose_routers_let_no_packet_through_play_through_the_relay() {
    let _real_time = crate::testing::real_time();
    peers_meet_through_a_relay(false, true);
}

/// A joiner given another code comes for another room, which no host holds. One given the room
/// alone (as one on the way to the relay sees it) is paired with the host and refused at the
/// handshake, never taking the joiner's place; a joiner given the code then joins.
#[test]
fn a_stranger_who_has_the_room_but_not_the_code_is_refused_at_the_handshake() {
    let _real_time = crate::testing::real_time();
    let logged = crate::testing::logged();
    let (relay, at) = relay();
    let (mut host, mut joiner) = relayed(&at, Game::default);
    let other: Code = "0000-0000-0000-0000".parse().expect("a code");
    let address = relay.address();
    let stranger = thread::spawn(move || {
        assert!(matches!(
            gg_relay::join(address, other.room()),
            Ok(Joined::Unhosted)
        ));
        let mut paired = loop {
            match gg_relay::join(address, TESTED.room()).expect("joined") {
                Joined::Paired(paired) => break paired,
                Joined::Unhosted => thread::sleep(Duration::from_millis(20)),
            }
        };
        paired
            .stream
            .set_read_timeout(Some(LOST))
            .expect("a timeout");
        secure::handshake(&mut paired.stream, &other, false).err()
    });
    let start = Instant::now();
    let mut refused = 0;
    while refused == 0 {
        assert!(start.elapsed() < Duration::from_secs(10), "never refused");
        host.turn();
        assert!(host.net().link.is_none());
        let codes = logged.codes.try_iter();
        refused += codes.filter(|code| code == "net.refused").count();
        thread::sleep(Duration::from_millis(1));
    }
    let error = stranger.join().expect("the stranger's").expect("refused");
    assert_eq!(error.kind(), ErrorKind::UnexpectedEof, "{error}");
    join(&mut host, &mut joiner);
    relay.stop();
}

fn until_phase(peer: &mut Peer, done: impl Fn(Option<SessionPhase>) -> bool) {
    let start = Instant::now();
    while !done(peer.phase()) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{:?}",
            peer.phase()
        );
        peer.turn();
        thread::sleep(Duration::from_millis(1));
    }
}

/// Runs started alone, partway through, host a session through their relay and join it by the
/// host's code, as a game's UI starts them. A joiner given a code no run hosts waits, unhosted,
/// until it leaves; then the peers play in step, worlds hashing alike. A leaving joiner plays on
/// alone, its host waiting for another; a leaving host has its joiner lose it.
fn runs_alone_host_and_join_by_the_code(rollback: bool) {
    let (relay, at) = relay();
    let networked = Rc::new(RefCell::new(Vec::new()));
    let game = || Game {
        rollback,
        networked: networked.clone(),
        ..Game::default()
    };
    let (mut host, mut joiner) = (Peer::alone(1, game(), &at), Peer::alone(2, game(), &at));
    for _ in 0..5 {
        host.turn();
        joiner.turn();
    }
    assert_eq!((host.next, joiner.next), (5, 5));
    assert!(host.phase().is_none() && !host.cx.networked());
    host.cx.host().expect("it hosts");
    let busy = host.cx.host().expect_err("one session at a time");
    assert_eq!(busy.code(), "net.busy");
    host.turn();
    assert!(host.cx.networked());
    assert_eq!(host.cx.player(), Some(0));
    assert_eq!(host.phase(), Some(SessionPhase::Waiting));
    assert_eq!(host.next, 6);
    let code = host.cx.session().expect("a session").code().to_owned();
    // A code no run hosts, until the joiner leaves it.
    joiner.cx.join("0000 0000 0000 0000").expect("a code");
    until_phase(&mut joiner, |phase| phase == Some(SessionPhase::Unhosted));
    assert_eq!(joiner.cx.player(), Some(1));
    let waited = joiner.next;
    joiner.cx.leave();
    joiner.turn();
    assert!(joiner.phase().is_none() && !joiner.cx.networked());
    assert_eq!((joiner.cx.player(), joiner.next), (None, waited + 1));
    // The code as a player might type it.
    joiner
        .cx
        .join(&code.to_lowercase().replace('-', " "))
        .expect("the code");
    until(&mut host, &mut joiner, |host, joiner| {
        let playing = Some(SessionPhase::Playing);
        host.phase() == playing && joiner.phase() == playing
    });
    assert_eq!(
        joiner.cx.session().map(SessionStatus::code),
        Some(code.as_str())
    );
    assert_eq!(networked.borrow().last(), Some(&("restored", true)));
    play(&mut host, &mut joiner, 30);
    let at = sent(&mut host, &mut joiner) + 1;
    settle(&mut host, &mut joiner, at);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let [ours, theirs] = host.score();
    assert!(ours > 0 && theirs > 0, "{ours} {theirs}");
    joiner.cx.leave();
    joiner.turn();
    assert!(joiner.phase().is_none() && joiner.cx.player().is_none());
    let next = joiner.next;
    joiner.turn();
    assert_eq!(joiner.next, next + 1);
    until_phase(&mut host, |phase| phase == Some(SessionPhase::Waiting));
    joiner.cx.join(&code).expect("the code");
    until(&mut host, &mut joiner, |_, joiner| {
        joiner.phase() == Some(SessionPhase::Playing)
    });
    host.cx.leave();
    host.turn();
    assert!(host.phase().is_none() && !host.cx.networked());
    until_phase(&mut joiner, |phase| phase == Some(SessionPhase::Lost));
    relay.stop();
}

#[test]
fn runs_alone_host_and_join_by_the_code_in_lockstep() {
    let _real_time = crate::testing::real_time();
    runs_alone_host_and_join_by_the_code(false);
}

#[test]
fn runs_alone_host_and_join_by_the_code_predicting() {
    let _real_time = crate::testing::real_time();
    runs_alone_host_and_join_by_the_code(true);
}

/// A thread that connects through a relay notes that no host holds its room, and ends once what it
/// looks for is gone (as a leaving joiner's: its channel closes), coming for it no more.
#[test]
fn a_thread_that_connects_ends_with_its_session() {
    let _real_time = crate::testing::real_time();
    let (relay, at) = relay();
    let looking = Looking::default();
    let host = "the lockstep host";
    let connecting = connect(Toward::Relay(at), host, Duration::ZERO, TESTED, &looking);
    let start = Instant::now();
    while !matches!(looking.heard.get(), Answer::Unhosted) {
        assert!(start.elapsed() < Duration::from_secs(10), "never unhosted");
        thread::sleep(Duration::from_millis(1));
    }
    drop(looking);
    let ended = connecting.recv_timeout(Duration::from_secs(5));
    assert!(matches!(ended, Err(RecvTimeoutError::Disconnected)));
    relay.stop();
}

/// A run starts a session of its own only where it can, saying why not: no relay, a recorded run,
/// or a code that isn't one (the error doesn't repeat it). A joiner whose relay can't be reached
/// says so as it tries.
#[test]
fn a_run_starts_a_session_of_its_own_only_where_it_can() {
    let _real_time = crate::testing::real_time();
    let mut nowhere = Peer::alone(1, Game::default(), "127.0.0.1:1");
    nowhere.cx.own.relay = None;
    let no_relay = nowhere.cx.host().expect_err("no relay");
    assert_eq!(no_relay.code(), "net.no_relay");
    nowhere.cx.own.relay = Some("127.0.0.1:1".to_owned());
    nowhere.cx.own.recorded = true;
    assert_eq!(
        nowhere.cx.host().expect_err("recorded").code(),
        "net.recorded"
    );
    nowhere.cx.own.recorded = false;
    let typed = "7KQ2-9XMB-4TZD-P0WU";
    let bad = nowhere.cx.join(typed).expect_err("U isn't a symbol");
    assert_eq!(bad.code(), "net.code");
    assert!(!format!("{bad:#}").contains("7KQ2"), "{bad:#}");
    assert!(nowhere.cx.join("7KQ2-9XMB").is_err());
    nowhere.cx.join("7KQ2-9XMB-4TZD-P0WH").expect("a code");
    until_phase(&mut nowhere, |phase| {
        matches!(phase, Some(SessionPhase::Unreachable { .. }))
    });
}

/// Takes a name's first IPv4 address wherever its resolver lists it, else its first.
#[test]
fn a_name_s_ipv4_address_is_taken_first() {
    let at = |text: &str| text.parse::<SocketAddr>().expect("an address");
    let (v6, v4) = (at("[2001:db8::7]:7800"), at("203.0.113.7:7800"));
    let other = at("[2001:db8::8]:7800");
    assert_eq!(preferred(&[v6, v4, at("198.51.100.1:7800")]), Some(v4));
    assert_eq!(preferred(&[v6, other]), Some(v6));
    assert_eq!(preferred(&[]), None);
    assert_eq!(
        resolve("127.0.0.1:7800").expect("resolved"),
        at("127.0.0.1:7800")
    );
}

/// A session whose relay is replaced at its address goes on through the new one: both peers lose
/// their link, the host registers again within [`REGISTER`], and the joiner joins again.
#[test]
fn peers_meet_again_through_a_relay_replaced() {
    let _real_time = crate::testing::real_time();
    let (relay, at) = relay();
    let (mut host, mut joiner) = relayed(&at, Game::default);
    join(&mut host, &mut joiner);
    relay.stop();
    let again = gg_relay::serve(&at, gg_relay::Limits::default()).expect("the same address");
    let start = Instant::now();
    while host.net().link.is_some() || joiner.net().link.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10), "never lost");
        host.turn();
        joiner.turn();
        thread::sleep(Duration::from_millis(1));
    }
    join(&mut host, &mut joiner);
    again.stop();
}

/// A link takes the packets the other end sealed, each once: a repeat, one sealed under another
/// link's keys, one under a nonce of the stream's and one in plain text, each from the other end's
/// socket, are dropped and warned of once (`net.forged`).
#[test]
fn a_link_takes_the_other_end_s_packets_alone_each_once() {
    let _real_time = crate::testing::real_time();
    let logged = crate::testing::logged();
    let [(made, made_keys), (taken, taken_keys)] = secure::linked();
    let mut ours = Link::new(taken, taken_keys, true, None).expect("a link");
    let mut theirs = Link::new(made, made_keys, false, None).expect("a link");
    ours.packets_from(theirs.port());
    theirs.packets_from(ours.port());
    let (socket, to) = (theirs.udp.try_clone().expect("a socket"), theirs.other);
    let send = |packet: &[u8]| {
        socket
            .send_to(packet, to.expect("an address"))
            .expect("sent");
    };
    let mut sealed = Vec::new();
    secure::seal_packet(&theirs.keys, 0, b"a stream's nonce", &mut sealed);
    send(&sealed);
    theirs.send_packet(b"one");
    send(&theirs.sealed);
    let [(_, other), _] = secure::linked();
    secure::seal_packet(&other, theirs.nonce, b"another link's", &mut sealed);
    send(&sealed);
    send(b"a 0 0 0 - 0 - -\n");
    theirs.send_packet(b"two");
    let (mut buffer, mut random) = (vec![0; 1 << 16], 0);
    let mut taken = Vec::new();
    let start = Instant::now();
    while taken.len() < 2 {
        assert!(start.elapsed() < Duration::from_secs(10), "{taken:?}");
        taken.extend(ours.take(&mut buffer, None, &mut random, Instant::now()).1);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(taken, [b"one".to_vec(), b"two".to_vec()]);
    let forged = logged.codes.try_iter().filter(|code| code == "net.forged");
    assert_eq!(forged.count(), 1);
}

/// What each end counts it sent, on its stream and in packets, the other counts taken: stream bytes
/// as sealed (more than its lines') and packets with their bytes.
#[test]
fn each_end_of_a_link_counts_what_the_other_sent() {
    let _real_time = crate::testing::real_time();
    let [(made, made_keys), (taken, taken_keys)] = secure::linked();
    let mut ends = [
        Link::new(taken, taken_keys, true, None).expect("a link"),
        Link::new(made, made_keys, false, None).expect("a link"),
    ];
    let ports = [ends[0].port(), ends[1].port()];
    ends[0].packets_from(ports[1]);
    ends[1].packets_from(ports[0]);
    let lines = "t 1\n".repeat(100);
    for (sends, end) in [1, 2].into_iter().zip(&mut ends) {
        for _ in 0..sends {
            end.send(lines.clone()).expect("sent");
            end.send_packet(lines.as_bytes());
        }
    }
    let (mut buffer, mut random) = (vec![0; 1 << 16], 0);
    let ways = |ends: &[Link; 2]| {
        [(0, 1), (1, 0)].map(|(from, to)| {
            let [from, to] = [&ends[from].traffic, &ends[to].traffic];
            let stream = [&from.written, &to.read].map(|bytes| bytes.load(Ordering::Relaxed));
            let packets = [from.sent[DIRECT], to.taken[DIRECT]].map(|c| (c.packets, c.bytes));
            (stream, packets)
        })
    };
    // Until both ways are as asserted below; counts that match alone prove nothing: they match at
    // zero, before the links' threads have written.
    let done = |sends: u64, (stream, packets): ([u64; 2], [(u64, u64); 2])| {
        stream[0] == stream[1]
            && stream[0] > sends * lines.len() as u64
            && packets[0] == packets[1]
            && packets[0].0 == sends
    };
    let start = Instant::now();
    let mut now = ways(&ends);
    while !([1, 2].into_iter().zip(now)).all(|(sends, way)| done(sends, way))
        && start.elapsed() < Duration::from_secs(5)
    {
        for end in &mut ends {
            end.take(&mut buffer, None, &mut random, Instant::now());
        }
        thread::sleep(Duration::from_millis(1));
        now = ways(&ends);
    }
    for (sends, (stream, packets)) in [1, 2].into_iter().zip(now) {
        assert_eq!(stream[0], stream[1], "the stream's bytes");
        assert!(stream[0] > sends * lines.len() as u64, "sealed: {stream:?}");
        assert_eq!(packets[0], packets[1], "the packets and their bytes");
        assert_eq!(packets[0].0, sends);
    }
}

/// What a link writes on its stream is sealed: its bytes hold none of its lines, which the other
/// end's keys open.
#[test]
fn a_link_s_stream_holds_none_of_its_lines() {
    let _real_time = crate::testing::real_time();
    let [(mut made, made_keys), (taken, taken_keys)] = secure::linked();
    let mut link = Link::new(taken, taken_keys, true, None).expect("a link");
    let text = "world 1234 1238 2 50124\ngg-snapshot 2\nend\n";
    link.send(text.to_owned()).expect("sent");
    let mut bytes = vec![0; 2 + text.len() + 16];
    made.set_read_timeout(Some(LOST)).expect("a timeout");
    made.read_exact(&mut bytes).expect("read");
    for line in text.lines() {
        let word = line.split(' ').next().expect("a word").as_bytes();
        let shows = bytes.windows(word.len()).any(|bytes| bytes == word);
        assert!(!shows, "{line}");
    }
    let mut opened = Opened::new(&bytes[..], Arc::new(made_keys));
    let mut read = String::new();
    opened.read_to_string(&mut read).expect("opened");
    assert_eq!(read, text);
}

/// A non-peer connection to `host`'s session, given its code: its stream once the handshake is done
/// (the host turning meanwhile), and what seals writes.
fn stranger(host: &mut Peer) -> (TcpStream, Sealer) {
    let address = host.net().listening.expect("listening");
    let made = thread::spawn(move || {
        let mut stream = TcpStream::connect(address)?;
        secure::handshake(&mut stream, &TESTED, false).map(|keys| (stream, keys))
    });
    while !made.is_finished() {
        host.turn();
        thread::sleep(Duration::from_millis(1));
    }
    let (stream, keys) = made.join().expect("made").expect("a handshake");
    (stream, Sealer::new(Arc::new(keys)))
}

/// A line as long as [`MAX_LINE`] is read; a longer one, as from a connection that never ends its
/// line, drops the connection before its hello, freeing the joiner's place.
#[test]
fn a_line_past_the_most_drops_its_connection() {
    let _real_time = crate::testing::real_time();
    let mut most = vec![b'x'; MAX_LINE];
    most.push(b'\n');
    let read = read_line(&mut &most[..]).expect("a line");
    assert_eq!(read.map(|line| line.len()), Some(MAX_LINE));

    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let (mut stranger, mut sealer) = stranger(&mut host);
    // Written as the host reads it, the connection held open after.
    let writer = thread::spawn(move || {
        let _ = sealer.write(&mut stranger, &vec![b'x'; MAX_LINE + 1]);
        stranger
    });
    let start = Instant::now();
    while !logged.try_iter().any(|code| code == "net.lost") {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the connection was never dropped"
        );
        host.turn();
        thread::sleep(Duration::from_millis(1));
    }
    let _stranger = writer.join().expect("written");
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
}

/// A connection that says no hello within [`HELLO`] of the host's taking it is lost, freeing the
/// joiner's place.
#[test]
fn a_connection_that_says_no_hello_is_lost() {
    let _real_time = crate::testing::real_time();
    let logged = crate::testing::logged();
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let _stranger = stranger(&mut host);
    let start = Instant::now();
    while host.net().link.is_none() {
        assert!(start.elapsed() < Duration::from_secs(10), "never taken");
        host.turn();
        thread::sleep(Duration::from_millis(1));
    }
    for _ in 0..10 {
        host.turn();
    }
    assert!(host.net().link.is_some(), "kept until its hello's time");
    // As though that time had gone by since the host took it.
    let link = host.net().link.as_mut().expect("a link");
    link.unhello = Instant::now().checked_sub(HELLO);
    host.turn();
    assert!(host.net().link.is_none());
    assert!(logged.codes.try_iter().any(|code| code == "net.lost"));
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
}

/// A peer that says hello and then nothing (cut off, or stopped in a debugger) is lost once nothing
/// has come by its link for the link's silence, the host playing on alone; until then it's kept.
#[test]
fn a_peer_nothing_comes_from_is_lost() {
    let _real_time = crate::testing::real_time();
    let logged = crate::testing::logged();
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let (mut stranger, mut sealer) = stranger(&mut host);
    let net = host.net();
    let line = hello(net.protocol(), net.app, &net.build, &net.layers, 9);
    sealer
        .write(&mut stranger, line.as_bytes())
        .expect("said hello");
    let start = Instant::now();
    while host.net().from.is_none() {
        assert!(start.elapsed() < Duration::from_secs(10), "never taken");
        host.turn();
        thread::sleep(Duration::from_millis(1));
    }
    // Its hello came a few turns ago.
    host.net().link.as_mut().expect("a link").silence = Duration::from_millis(300);
    host.turn();
    assert!(host.net().link.is_some(), "kept until its silence");
    let start = Instant::now();
    while host.net().link.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10), "never lost");
        host.turn();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(logged.codes.try_iter().any(|code| code == "net.lost"));
    let at = host.next;
    for _ in 0..10 {
        host.turn();
    }
    assert!(host.next > at, "the host plays on alone");
}

/// A peer whose frames stop longer than its link's silence (restoring a large world, a script's
/// breakpoint) is kept, as its link's writing thread says it's there; the peers go on in step once
/// its frames resume.
#[test]
fn a_peer_whose_frames_stop_is_kept_by_its_link_s_thread() {
    let _real_time = crate::testing::real_time();
    let logged = crate::testing::logged();
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    play(&mut host, &mut joiner, 10);
    // Past `KEEP`, after which the joiner's thread writes `k`.
    let silence = KEEP * 5 / 2;
    host.net().link.as_mut().expect("a link").silence = silence;
    let stopped = Instant::now();
    while stopped.elapsed() < silence + KEEP {
        host.turn();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(host.net().link.is_some(), "the joiner was lost");
    play(&mut host, &mut joiner, 10);
    let at = sent(&mut host, &mut joiner) + 1;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.score(), joiner.score());
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let codes: Vec<String> = logged.codes.try_iter().collect();
    assert!(!codes.iter().any(|code| code == "net.lost"), "{codes:?}");
}

/// A joiner of another build, whose simulation may differ, is refused, its hello naming both
/// builds: one of the same source and another package too.
#[test]
fn a_joiner_of_another_build_is_refused() {
    let _real_time = crate::testing::real_time();
    let ours = [
        LOCKSTEP,
        "Tested",
        "0.1.0 debug",
        "0123456789abcdef",
        "Game",
    ];
    let theirs = hello(ours[0], ours[1], "fedcba9876543210", ours[4], 9);
    let theirs = theirs.trim_end().replace(&engine(), ours[2]);
    let why = port_of(&theirs, ours, "the host").expect_err("refused");
    assert!(
        why.contains("build fedcba9876543210") && why.contains("build 0123456789abcdef"),
        "{why}"
    );

    let logged = crate::testing::logged();
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let address = host.net().listening.expect("listening").to_string();
    let layers = host.net().layers.clone();
    let layers: Vec<&str> = layers.split(' ').collect();
    let join = Net::Join(crate::args::Toward::Address(address));
    let mut other = Lockstep::start(&join, TESTED, "Tested", BUILD, &layers).expect("started");
    // Of the same source, as `BUILD`, and another package.
    other.build = "0123456789abcdef-0000000000000000".to_owned();
    let start = Instant::now();
    let mut refused = 0;
    while refused < 2 {
        assert!(start.elapsed() < Duration::from_secs(10), "never refused");
        host.turn();
        other.turn(0);
        refused += logged
            .codes
            .try_iter()
            .filter(|code| code == "net.refused")
            .count();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(other.link.is_none() && host.net().link.is_none());
}

/// A run that names its source is named by it and its package's bytes, so two runs of one source
/// and package are alike wherever their packages are and one of another package isn't; or by its
/// source alone if it reads the disk. Its hello names it so, not by its executable's digest.
#[test]
fn a_run_naming_its_source_is_named_by_it_and_its_package() {
    use crate::package::{Kind, Package, Writer};

    let dir = std::env::temp_dir().join(format!("gg-net-build-{}", std::process::id()));
    let files = |name: &str, scene: &[u8]| {
        let mut writer = Writer::new(".", Some("scenes/arena.ron"), None);
        (writer.add("scenes/arena.ron", None, Kind::File, scene.to_vec())).expect("added");
        let path = dir.join(name).join("game.ggpk");
        writer.write(&path).expect("written");
        let package = Package::open(&path).expect("opened");
        Files::package(Arc::new(package), dir.join(name))
    };
    let source = Some("3f8a6c2e9b1d4057");
    let named = |files| Build { source, files }.name().expect("named");
    let ours = files("ours", b"(speed: 2)");
    let bytes = std::fs::read(dir.join("ours").join("game.ggpk")).expect("read");
    let digest = crate::ecs::bytes_digest(&bytes);
    assert_eq!(
        named(ours.clone()),
        format!("3f8a6c2e9b1d4057-{digest:016x}")
    );
    let apart = named(files("apart", b"(speed: 2)"));
    assert_eq!(apart, named(ours.clone()), "the same bytes elsewhere");
    let theirs = named(files("theirs", b"(speed: 3)"));
    assert_ne!(theirs, named(ours.clone()), "another scene");
    assert_eq!(named(Files::default()), "3f8a6c2e9b1d4057--");
    let unnamed = Build::default().name().expect("named");
    assert_eq!(unnamed, executable().expect("digested"));

    let net = Net::Host(crate::args::Listen::at("127.0.0.1:0"));
    let build = Build {
        source,
        files: ours,
    };
    let session = Session::start(&net, Some(TESTED), "Tested", &build, &[]).expect("started");
    let Session::Peer(peer) = &session else {
        panic!("a peer");
    };
    assert_eq!(peer.build, format!("3f8a6c2e9b1d4057-{digest:016x}"));
    // Closed first: Windows removes no open file.
    drop((session, build));
    std::fs::remove_dir_all(&dir).expect("removed");
}

/// A peer's command naming the other's player is refused by both peers, which go on in step; its
/// command for its own player applies.
#[test]
fn a_peer_s_command_for_the_other_s_player_is_refused_by_both() {
    let _real_time = crate::testing::real_time();
    let logged = crate::testing::logged();
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    joiner.stack_commands(Command::Add {
        player: 0,
        points: 100,
    });
    joiner.stack_commands(Command::Add {
        player: 1,
        points: 10,
    });
    let at = sent(&mut host, &mut joiner) + 2;
    meet(&mut host, &mut joiner, at);
    assert_eq!((host.score(), joiner.score()), ([0, 10], [0, 10]));
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let refused = codes.iter().filter(|code| *code == "net.bad_command");
    assert_eq!(refused.count(), 2, "one by each peer: {codes:?}");
    assert!(!codes.iter().any(|code| code == "net.desync"), "{codes:?}");
}

/// An edit the joiner offers, and a save of a scene file the host's game loaded, apply on both
/// peers before the same tick, each answered to its offerer. The joiner, which doesn't look at
/// files, applies the save as the host does and keeps the file's text.
#[cfg(feature = "dev")]
#[test]
fn offered_edits_and_scene_saves_apply_on_both_peers() {
    use std::fs;

    use crate::Transform;
    use crate::dirs::tests::Scratch;

    let _real_time = crate::testing::real_time();
    let dir = Scratch::new("net-scene");
    fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
    let path = dir.0.join("scenes").join("level.ron");
    // Seeded apart from the peers, whose worlds would have its entity's ID already.
    let mut file = World::seeded(3);
    let a = file.spawn(Transform::IDENTITY);
    fs::write(&path, file.save_scene(&[a]).expect("saved")).expect("written");
    let game = Game {
        scene: Some(path.clone()),
        ..Game::default()
    };
    let mut host = Peer::host(Context::seeded(1).root(&dir.0), game, 10);
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    assert_eq!(joiner.cx.scenes.files, host.cx.scenes.files);

    let edit = joiner.set("agent", 0, 1000);
    let ticket = joiner.net().offer(edit);
    until(&mut host, &mut joiner, |_, joiner| {
        joiner.net().is_done(ticket)
    });
    let (_, done) = joiner.net().done(ticket).expect("done");
    assert!(matches!(done, Ok(Done::Applied(_))), "{done:?}");
    let at = host.next.max(joiner.next) + 1;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.score(), [1000, 0]);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());

    // Saved before either peer's first look, which finds it changed from the text it loaded as.
    let moved = Transform {
        translation: DVec3::new(1.0, 2.0, 3.0),
        ..Transform::IDENTITY
    };
    file.insert(a, moved).expect("alive");
    let saved = file.save_scene(&[a]).expect("saved");
    fs::write(&path, &saved).expect("written");
    crate::scene_files::look(&mut joiner.cx, &mut joiner.stack);
    assert!(joiner.net().offered.is_empty());
    crate::scene_files::look(&mut host.cx, &mut host.stack);
    assert_eq!(host.net().offered.len(), 1);
    let at = |peer: &Peer| *peer.cx.world().query::<&Transform>().next().expect("one");
    until(&mut host, &mut joiner, |host, joiner| {
        at(host) == moved && at(joiner) == moved
    });
    let met = host.next.max(joiner.next) + 1;
    meet(&mut host, &mut joiner, met);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    for peer in [&host, &joiner] {
        assert_eq!(peer.cx.scenes.files[0].text, saved);
        let logs = peer.cx.authors.logs(peer.cx.world());
        let authors: Vec<&str> = logs.map(|(author, _)| author).collect();
        assert!(authors.contains(&"scenes/level.ron"), "{authors:?}");
    }
}

/// A joiner runs its host's scripts, which come with its world, whatever its own files hold; a
/// script's save, which only the host looks at, reloads on both peers before the same tick.
#[cfg(feature = "dev")]
#[test]
fn a_joiner_runs_its_host_s_scripts_which_reload_on_both_peers_as_they_re_saved() {
    use std::fs;

    use crate::dirs::tests::Scratch;

    let _real_time = crate::testing::real_time();
    let spin = |fields: &str, step: u32| {
        format!(
            "local Spin = gg.declare(\"Spin\", {{ count = 0{fields} }})\n\
                 function Spin.tick(entity, dt)\n\
                     entity[Spin.count] = entity[Spin.count] + {step}\n\
                 end\n\
                 return Spin\n"
        )
    };
    let game = |name: &str, text: &str| {
        let dir = Scratch::new(name);
        fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
        let path = dir.0.join("scripts").join("spin.luau");
        fs::write(&path, text).expect("written");
        let scripts = crate::Scripts::new().expect("Luau starts");
        let game = Game {
            script: Some((scripts, path.clone())),
            ..Game::default()
        };
        (dir, path, game)
    };
    let (host_dir, path, ours) = game("net-script-host", &spin("", 1));
    let mut host = Peer::host(Context::seeded(1).root(&host_dir.0), ours, 10);
    let (joiner_dir, _, theirs) = game("net-script-joiner", &spin("", 100));
    let address = host.net().listening.expect("listening").to_string();
    let cx = Context::seeded(2).root(&joiner_dir.0);
    let mut joiner = Peer::new(
        &Net::Join(crate::args::Toward::Address(address)),
        cx,
        theirs,
    );
    join(&mut host, &mut joiner);
    assert_eq!(joiner.cx.hosts.applied(), host.cx.hosts.applied());
    let at = host.next.max(joiner.next) + 5;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());

    // Saved with a field more, which the joiner doesn't look for.
    let saved = spin(", turned = true", 2);
    fs::write(&path, &saved).expect("written");
    crate::luau::look(&mut joiner.cx, &mut joiner.stack);
    assert!(joiner.net().offered.is_empty());
    crate::luau::look(&mut host.cx, &mut host.stack);
    assert_eq!(host.net().offered.len(), 1);
    let turned = |peer: &Peer| {
        let world = peer.cx.world();
        let spun = world.entities(Some("Spin")).expect("a type").next();
        world
            .get_field(spun.expect("one"), "Spin", "turned")
            .is_ok()
    };
    until(&mut host, &mut joiner, |host, joiner| {
        turned(host) && turned(joiner)
    });
    let met = host.next.max(joiner.next) + 5;
    meet(&mut host, &mut joiner, met);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let applied = vec![("scripts/spin.luau".to_owned(), saved)];
    assert_eq!(host.cx.hosts.applied(), applied);
    assert_eq!(joiner.cx.hosts.applied(), applied);
}

/// A joiner without its host's mod runs the host's, which comes with its world.
#[test]
fn a_joiner_without_its_host_s_mod_runs_the_host_s() {
    use std::fs;

    use crate::dirs::tests::Scratch;

    let _real_time = crate::testing::real_time();
    let installed = Scratch::new("net-mods-host");
    let folder = installed.0.join("tally");
    fs::create_dir_all(&folder).expect("a directory");
    let manifest = "(name: \"tally\", version: \"1.0.0\", game: \"Tested\", \
            capabilities: [\"read:Score\"], scripts: [\"tally.luau\"])";
    fs::write(folder.join("mod.ron"), manifest).expect("written");
    let script = "local Tally = gg.declare(\"Tally\", { count = 0 }, { on = \"Score\" })\n\
            function Tally.tick(entity, dt)\n\
                entity[Tally.count] = entity[Tally.count] + 1\n\
            end\n\
            return Tally\n";
    fs::write(folder.join("tally.luau"), script).expect("written");
    let none = Scratch::new("net-mods-joiner");
    let modded = |dir: &Scratch, seed| {
        let mut cx = Context::seeded(seed);
        cx.mods.game = "Tested".to_owned();
        cx.mods.dir = Some(dir.0.clone());
        let game = Game {
            modded: true,
            ..Game::default()
        };
        (cx, game)
    };
    let (cx, game) = modded(&installed, 1);
    let mut host = Peer::host(cx, game, 10);
    let address = host.net().listening.expect("listening").to_string();
    let (cx, game) = modded(&none, 2);
    let mut joiner = Peer::new(&Net::Join(crate::args::Toward::Address(address)), cx, game);
    assert!(joiner.cx.mods.applied().is_empty());
    join(&mut host, &mut joiner);
    assert_eq!(joiner.cx.mods.applied(), host.cx.mods.applied());
    assert_eq!(host.cx.mods.applied().len(), 1);
    let at = host.next.max(joiner.next) + 5;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let world = joiner.cx.world();
    let tallied = world.entities(Some("Tally")).expect("a type").next();
    let count = world.get_field(tallied.expect("one"), "Tally", "count");
    assert_eq!(
        count.expect("its field"),
        crate::reflect::Value::F64(at as f64)
    );
}

/// Patches `gg run` gives both peers alike. The host's, alone, applies before the tick its delay on
/// from its next. The joiner's, got before it joined, applies before the first tick of its host's
/// world. Once joined, the host's next applies on both before the tick it chose; the joiner waits
/// there for its own, or, in rollback, is there or past it on predicted inputs and rolls back once
/// its patch comes. Each is answered with the tick it applied before, here a refusal: no process
/// takes a test's tables.
#[cfg(feature = "dev")]
#[test]
fn patches_apply_on_both_peers_before_the_ticks_the_host_chooses() {
    let _real_time = crate::testing::real_time();
    for rollback in [false, true] {
        patches_apply_before_the_ticks_the_host_chooses(rollback);
    }
}

#[cfg(feature = "dev")]
fn patches_apply_before_the_ticks_the_host_chooses(rollback: bool) {
    fn patch(peer: &mut Peer, table: subsecond::JumpTable) -> Ticket {
        let Ok(ticket) = peer.stack.offer_patch(table) else {
            panic!("a peer offers its patches");
        };
        ticket
    }
    let table = || subsecond::JumpTable {
        lib: std::env::temp_dir().join("gg-net-patch.dll"),
        map: Default::default(),
        aslr_reference: u64::MAX,
        new_base_address: 0,
        ifunc_count: 0,
    };
    let refused = |done: Result<Done>| {
        let code = done.err().map(|error| error.code());
        assert_eq!(code, Some("api.patch"));
    };
    let game = Game {
        rollback,
        ..Game::default()
    };
    let mut host = Peer::host(Context::seeded(1), game, 10);
    let first = patch(&mut host, table());
    let chosen = host.next + host.net().delay;
    while host.next <= chosen {
        host.turn();
    }
    let (at, done) = host.net().done(first).expect("applied");
    assert_eq!(at, chosen);
    refused(done);

    // The joiner's, from before it joined: applies as the first tick of its host's world runs, once
    // the host's inputs for it, which follow the world, have come.
    let mut joiner = Peer::joiner(&mut host);
    let theirs = patch(&mut joiner, table());
    let world = host.next;
    join(&mut host, &mut joiner);
    until(&mut host, &mut joiner, |_, joiner| {
        joiner.net().is_done(theirs)
    });
    let (at, done) = joiner.net().done(theirs).expect("applied");
    assert_eq!(at, world);
    refused(done);

    // The host's next: the joiner waits for its own from `gg run` though it has the host's inputs
    // for the tick.
    let second = patch(&mut host, table());
    let due = |host: &mut Peer| host.net().patches.due.last().map(|&(_, at)| at);
    until(&mut host, &mut joiner, |host, _| due(host) != Some(UNSENT));
    let chosen = due(&mut host).expect("due");
    until(&mut host, &mut joiner, |_, joiner| joiner.next >= chosen);
    until(&mut host, &mut joiner, |host, joiner| {
        let inputs = joiner.net().theirs.get(&chosen);
        host.next > chosen && inputs.is_some_and(|inputs| inputs.whole)
    });
    if rollback {
        let next = joiner.next;
        let predicted = &joiner.net().predicted;
        let past = (chosen..next).all(|tick| predicted.contains_key(&tick));
        assert!(
            past,
            "run past it, to {next}, on the host's inputs predicted alone"
        );
    } else {
        assert_eq!(joiner.next, chosen);
    }
    let theirs = patch(&mut joiner, table());
    until(&mut host, &mut joiner, |host, joiner| {
        host.net().is_done(second) && joiner.net().is_done(theirs)
    });
    for (peer, ticket) in [(&mut host, second), (&mut joiner, theirs)] {
        let (at, done) = peer.net().done(ticket).expect("applied");
        assert_eq!(at, chosen);
        refused(done);
    }
    let at = host.next.max(joiner.next) + 1;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
}

impl Peer {
    fn stack_commands(&mut self, command: Command) {
        let queue = self.stack.commands_of(0).expect("the game queues commands");
        queue
            .push(&command.to_value())
            .expect("the game takes its commands");
    }
}

/// [`Painter`]'s command: spawns a sprite of this texture.
#[cfg(feature = "dev")]
#[derive(Reflect)]
struct Paint(crate::Texture);

/// Loads `assets/stone.png` under its context's root, attaches a sprite of it, and spawns a sprite
/// of each texture its commands give.
#[cfg(feature = "dev")]
#[derive(Default)]
struct Painter {
    stone: Option<crate::Texture>,
    commands: Vec<Paint>,
}

#[cfg(feature = "dev")]
impl Layer for Painter {
    fn load(&mut self, cx: &mut Context) {
        let root = cx.root.clone().expect("a root");
        let stone = cx.add_texture("stone", root.join("assets/stone.png"));
        self.stone = Some(stone.expect("a texture"));
    }

    fn attach(&mut self, cx: &mut Context) {
        self.commands.push(Paint(self.stone.expect("loaded")));
        self.tick(cx.world_mut(), Tick { number: 0 });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for Paint(texture) in self.commands.drain(..) {
            let sprite = crate::Sprite {
                texture: Some(texture),
                ..crate::Sprite::default()
            };
            world.spawn((crate::Transform::IDENTITY, sprite));
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }
}

/// A joiner whose disk holds none of its host's ID files (another machine's checkout, files not yet
/// committed) names its textures as its host does: those it loaded, as the host's world comes; one
/// both add after it joined, as the host's line says before the command naming it; and one only the
/// host loaded where text named it, which the joiner loads as it reads that command.
#[cfg(feature = "dev")]
#[test]
fn a_joiner_names_textures_as_its_host_does() {
    use std::fs;

    use crate::dirs::tests::Scratch;

    let _real_time = crate::testing::real_time();
    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    let dir = Scratch::new("net-named");
    let png =
        crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    let [ours, theirs] = ["host", "joiner"].map(|peer| dir.0.join(peer));
    for root in [&ours, &theirs] {
        fs::create_dir_all(root.join("assets")).expect("a directory");
        for name in ["stone.png", "late.png", "named.png"] {
            fs::write(root.join("assets").join(name), &png).expect("a PNG");
        }
    }
    let (stone, late, named) = ("5f3c9a1e07b2d846", "0a1b2c3d4e5f6071", "7e6d5c4b3a291807");
    for (name, id) in [("stone", stone), ("late", late), ("named", named)] {
        let meta = gg_api::asset::Meta {
            id: id.parse().expect("an ID"),
        };
        let file = ours.join("assets").join(format!("{name}.png.meta"));
        fs::write(file, meta.text()).expect("its ID file");
    }
    let peer = |net: &Net, mut cx: Context| {
        let mut stack = Stack::default();
        stack.push_layer(Painter::default());
        let names = stack.names().to_vec();
        stack.lockstep(Lockstep::start(net, TESTED, "Tested", BUILD, &names).expect("started"));
        stack.attach(&mut cx);
        Peer { stack, cx, next: 0 }
    };
    let mut host = peer(
        &Net::Host(crate::args::Listen::at("127.0.0.1:0")),
        Context::seeded(1).root(&ours),
    );
    host.turn();
    let address = host.net().listening.expect("listening").to_string();
    // As a joiner's run starts: its textures by path until its host's world comes.
    let mut cx = Context::seeded(2).root(&theirs);
    cx.name_as([]);
    let mut joiner = peer(&Net::Join(crate::args::Toward::Address(address)), cx);
    join(&mut host, &mut joiner);
    let textured = |peer: &Peer, path: &str| {
        let assets = peer.cx.world().assets();
        let texture = assets.texture(path)?;
        assets.name(texture).map(str::to_owned)
    };
    assert_eq!(
        textured(&joiner, "assets/stone.png").as_deref(),
        Some(stone)
    );

    let add = |peer: &mut Peer| {
        let root = peer.cx.root.clone().expect("a root");
        let added = peer.cx.add_texture("late", root.join("assets/late.png"));
        added.expect("a texture")
    };
    let paint = |peer: &mut Peer, texture| {
        let queue = peer.stack.commands_of(0);
        let queue = queue.expect("the painter queues commands");
        queue
            .push(&Paint(texture).to_value())
            .expect("the painter takes its commands");
    };
    let texture = add(&mut host);
    add(&mut joiner);
    let path = "assets/late.png";
    assert_eq!(textured(&joiner, path).as_deref(), Some(path));
    paint(&mut host, texture);
    let at = sent(&mut host, &mut joiner) + 1;
    meet(&mut host, &mut joiner, at);
    assert_eq!(textured(&joiner, path).as_deref(), Some(late));
    // One the host loaded where text named it, as through its tool API, which the joiner's layers
    // never add, loads there as the command naming it is read.
    host.cx
        .load_named([crate::assets::Asset::Texture(named)], false);
    let texture = host.cx.world().assets().texture(named);
    paint(&mut host, texture.expect("loaded"));
    let at = sent(&mut host, &mut joiner) + 1;
    meet(&mut host, &mut joiner, at);
    let path = "assets/named.png";
    assert_eq!(textured(&joiner, path).as_deref(), Some(named));
    let sprites = |peer: &Peer| peer.cx.world().query::<&crate::Sprite>().count();
    assert_eq!((sprites(&host), sprites(&joiner)), (3, 3));
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let codes: Vec<String> = logged.try_iter().collect();
    let parted = ["net.desync", "net.bad_command"];
    assert!(
        !codes.iter().any(|code| parted.contains(&code.as_str())),
        "{codes:?}"
    );
}

/// A scene file's save naming textures no layer added has its host load them before offering it, so
/// their names go out first; the joiner, with none of the ID files, loads each by the path the
/// host's name gives, or stands in for one it hasn't. A save naming a texture the host can't load
/// isn't offered, as the joiner might stand in for it.
#[cfg(feature = "dev")]
#[test]
fn a_joiner_loads_the_textures_its_hosts_scene_saves_name() {
    use std::fs;

    use crate::dirs::tests::Scratch;
    use crate::scene_files::tests::{look_now, save};

    let _real_time = crate::testing::real_time();
    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    let dir = Scratch::new("net-loaded");
    let png =
        crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    let [ours, theirs] = ["host", "joiner"].map(|peer| dir.0.join(peer));
    let (late, gone, set) = ("0a1b2c3d4e5f6071", "1d2c3b4a59687f0e", "2b3c4d5e6f708192");
    for (root, names) in [
        (&ours, &["late", "set", "gone"][..]),
        (&theirs, &["late", "set"]),
    ] {
        fs::create_dir_all(root.join("assets")).expect("a directory");
        fs::create_dir_all(root.join("scenes")).expect("a directory");
        for name in names {
            fs::write(root.join("assets").join(format!("{name}.png")), &png).expect("a PNG");
        }
    }
    for (name, id) in [("late", late), ("gone", gone), ("set", set)] {
        let meta = gg_api::asset::Meta {
            id: id.parse().expect("an ID"),
        };
        let file = ours.join("assets").join(format!("{name}.png.meta"));
        fs::write(file, meta.text()).expect("its ID file");
    }
    let scene = |names: &[&str]| {
        let mut text = "(\n    entities: {\n".to_owned();
        text += "        \"0000000000003000\": (\n            Name: (\"a\"),\n        ),\n";
        for (i, name) in names.iter().enumerate() {
            text += &format!(
                "        \"{:016x}\": (\n            Sprite: (\n                texture: \
                     Some(\"{name}\"),\n            ),\n        ),\n",
                0x3001 + i
            );
        }
        text + "    },\n)\n"
    };
    let path = ours.join("scenes/level.ron");
    fs::write(&path, scene(&[late])).expect("written");
    let game = Game {
        scene: Some(path.clone()),
        ..Game::default()
    };
    let mut host = Peer::host(Context::seeded(1).root(&ours), game, 10);
    let address = host.net().listening.expect("listening").to_string();
    let mut cx = Context::seeded(2).root(&theirs);
    cx.name_as([]);
    let mut joiner = Peer::new(
        &Net::Join(crate::args::Toward::Address(address)),
        cx,
        Game::default(),
    );
    join(&mut host, &mut joiner);

    save(&path, &scene(&["assets/none.png"]), 1);
    look_now(&mut host.cx, &mut host.stack);
    assert!(
        host.net().offered.is_empty(),
        "a texture the host can't load"
    );
    save(&path, &scene(&[late, gone]), 2);
    look_now(&mut host.cx, &mut host.stack);
    assert_eq!(host.net().offered.len(), 1);
    let sprites = |peer: &Peer| peer.cx.world().query::<&crate::Sprite>().count();
    until(&mut host, &mut joiner, |host, joiner| {
        sprites(host) == 2 && sprites(joiner) == 2
    });
    let at = host.next.max(joiner.next) + 1;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());

    host.cx
        .load_named([crate::assets::Asset::Texture(set)], false);
    let texture = host.cx.world().assets().texture(set).expect("loaded");
    let operation = Operation::Set {
        entity: "0000000000003001".parse().expect("an ID"),
        component: "Sprite".into(),
        path: "texture".to_owned(),
        value: Value::Option(Some(Box::new(Value::Texture(texture)))),
    };
    let edit = Edit::Apply {
        author: "agent".to_owned(),
        operations: vec![operation],
    };
    let ticket = host.net().offer(edit);
    until(&mut host, &mut joiner, |host, _| host.net().is_done(ticket));
    let (_, done) = host.net().done(ticket).expect("done");
    assert!(matches!(done, Ok(Done::Applied(_))), "{done:?}");
    let at = host.next.max(joiner.next) + 1;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let assets = joiner.cx.world().assets();
    let loaded = |name| {
        assets
            .texture(name)
            .and_then(|texture| assets.path(texture))
    };
    let paths = (loaded(late), loaded(set));
    assert_eq!(paths, (Some("assets/late.png"), Some("assets/set.png")));
    let codes: Vec<String> = logged.try_iter().collect();
    let parted = ["net.desync", "net.bad_command"];
    assert!(
        !codes.iter().any(|code| parted.contains(&code.as_str())),
        "{codes:?}"
    );
    let stood_in = codes.iter().filter(|code| *code == "asset.stand_in");
    assert_eq!(stood_in.count(), 1, "{codes:?}");
}

/// A peer keeps the other's inputs within a second of ticks past the first it hasn't had whole. A
/// packet holding inputs past that, as a hostile peer's may, or a tick no run reaches, is rejected
/// and loses the other peer; the ticks before it stay taken.
#[test]
fn a_peer_keeps_the_other_s_inputs_to_a_second() {
    let _real_time = crate::testing::real_time();
    let net = Net::Join(crate::args::Toward::Address("127.0.0.1:1".to_owned()));
    let mut peer = Lockstep::start(&net, TESTED, "Tested", BUILD, &[]).expect("started");
    let now = Instant::now();
    assert!(
        peer.read_ticks(b"a 0 0 0 - 0 - -\nt 0\nt 63\n", now)
            .is_some()
    );
    peer.came_whole();
    assert_eq!(peer.whole, 1);
    let far = [
        "a 0 0 0 - 0 - -\nt 2\nt 65\n",
        "a 0 18446744073709551615 0 - 0 - -\n",
    ];
    for far in far {
        assert!(peer.read_ticks(far.as_bytes(), now).is_none(), "{far}");
    }
    let kept: Vec<u64> = peer.theirs.keys().copied().collect();
    assert_eq!(kept, [0, 2, 63]);
}

/// Shows a texture.
#[derive(Reflect)]
struct Show(crate::Texture);

/// A tick's commands fill their layer's queue each alone: one that isn't a command, or that the
/// layer refuses, is refused and logged with its player, and those after it are queued still. A
/// server's client's name only assets the server has loaded, one naming another refused before
/// anything loads it; a peer's loads what it names, standing in for what can't be.
#[test]
fn commands_fill_their_queue_each_alone_a_client_s_naming_only_what_s_loaded() {
    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    let mut cx = Context::seeded(1);
    let missing = "assets/missing.png";
    let shows = format!("Struct([(\"0\", Texture(\"{missing}\"))])");
    let queued = |player: u8, remote: bool, text: &str| Queued {
        layer: 0,
        player: Some(player),
        remote,
        text: text.to_owned(),
    };
    let fill = |commands: Vec<Queued>, cx: &mut Context| {
        let merged = Merged {
            commands,
            ..Merged::default()
        };
        let mut queue: Vec<Show> = Vec::new();
        merged.fill(0, "Shows", &mut queue, cx);
        queue.len()
    };
    let loaded = |cx: &Context| cx.world().assets().has(&Asset::Texture(missing));
    let refused = [queued(1, true, "Struct("), queued(2, true, &shows)];
    assert_eq!(fill(refused.into(), &mut cx), 0);
    assert!(!loaded(&cx), "a client's command loads nothing");
    let codes: Vec<String> = logged.try_iter().collect();
    let bad = codes.iter().filter(|code| *code == "net.bad_command");
    assert_eq!(bad.count(), 1, "one for the layer and tick: {codes:?}");
    let taken = [
        queued(1, false, "UInt(3)"),
        queued(0, false, &shows),
        queued(2, true, &shows),
    ];
    assert_eq!(fill(taken.into(), &mut cx), 2);
    assert!(loaded(&cx), "a peer's command stands in for it");
}

/// A joiner and its host spawn entities into the same slots after it joins, as code that orders by
/// [`Entity`] needs: the host's authors' logs go as it sends its world, warning how many edits
/// went, and the slots they kept vacant for undos are freed, as the joiner's load frees them. No
/// undo reaches past the joiner's starting world.
#[test]
fn a_joiner_and_its_host_spawn_into_the_same_slots() {
    let _real_time = crate::testing::real_time();
    let (codes, logged) = mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(codes));
    let _logging = tracing::subscriber::set_default(subscriber);
    // Entities made and deleted between ticks, behind the session's back, as a peer's edits do.
    let edit = |peer: &mut Peer, operations: Vec<Operation>| {
        let edit = Edit::Apply {
            author: "editor".to_owned(),
            operations,
        };
        match peer.cx.apply_edit(edit).expect("applied") {
            Done::Applied(made) => made,
            Done::Stepped(_) => panic!("an apply"),
        }
    };
    let create = || Operation::Create {
        entity: None,
        parent: None,
        components: Vec::new(),
    };
    let spawned = |peer: &mut Peer| {
        let made = edit(peer, vec![create(), create(), create(), create()]);
        let world = peer.cx.world();
        let made = made.into_iter().map(|id| world.entity(id).expect("alive"));
        made.collect::<Vec<_>>()
    };
    let mut host = Peer::host(Context::seeded(1), Game::default(), 1);
    let gone = edit(&mut host, vec![create(), create(), create()]);
    let delete = gone.into_iter().map(|entity| Operation::Delete { entity });
    edit(&mut host, delete.collect());
    let mut joiner = Peer::joiner(&mut host);
    join(&mut host, &mut joiner);
    let at = host.next.max(joiner.next) + 1;
    meet(&mut host, &mut joiner, at);
    assert_eq!(host.cx.world().hash(), joiner.cx.world().hash());
    let undo = Edit::Undo {
        author: "editor".to_owned(),
    };
    let undone = host.cx.apply_edit(undo).expect("stepped");
    assert_eq!(undone, Done::Stepped(None));
    assert_eq!(spawned(&mut host), spawned(&mut joiner));
    let cleared = logged
        .try_iter()
        .filter(|code| code == "edits.history_cleared");
    assert_eq!(cleared.count(), 1);
}

/// A renewal, as a lockstep host's when it sends its joiner its world, refuses the entries of every
/// earlier log, a layer's (as the Inspector's) among them: its undo is refused, and its clear frees
/// nothing the renewal freed, so no two entities take one slot. It's an edit, so a recording keeps
/// it and a replay renews its world where the run did.
#[test]
fn a_renewal_refuses_every_log_s_entries_from_before() {
    let mut cx = Context::seeded(1);
    let world = cx.world_mut();
    let gone = world.spawn(crate::Transform::IDENTITY);
    let id = world.id(gone).expect("an ID");
    let mut log = crate::ecs::Log::default();
    let delete = Operation::Delete { entity: id };
    log.apply(world, delete).expect("deleted");
    let renew = Edit::from_value(&Edit::Renew.to_value()).expect("an edit");
    cx.apply_edit(renew).expect("renewed");
    let world = cx.world_mut();
    assert!(log.undo(world).is_err(), "refused");
    log.clear(world);
    let [a, b] = [0, 1].map(|_| world.spawn(crate::Transform::IDENTITY));
    assert_ne!(a.slot(), b.slot());
    assert!(world.entity(id).is_none());
}
