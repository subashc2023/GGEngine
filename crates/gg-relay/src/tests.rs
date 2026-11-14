use std::net::{IpAddr, Shutdown, UdpSocket};
use std::thread;

use super::relay::{key, serve_within};
use super::*;

const ROOM: Room = Room([7; 16]);

/// How long a relay waits for a first line or a host's take in the tests that wait it out, in place
/// of [`ANSWER`] or [`TAKE`].
const WAITED: Duration = Duration::from_secs(1);

/// The default limits, but at most `most` connections.
fn most(most: usize) -> Limits {
    Limits {
        most,
        ..Limits::default()
    }
}

fn relay(limits: Limits) -> Relay {
    serve("127.0.0.1:0", limits).expect("a relay")
}

/// Waits until `relay` holds nothing, as it keeps nothing of what has gone, failing after 5 s.
fn until_empty(relay: &Relay) {
    let start = Instant::now();
    while relay.holds() != [0; 7] {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            relay.holds()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// A UDP socket on the loopback, its reads waiting up to 300 ms.
fn socket() -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("bound");
    socket
        .set_read_timeout(Some(Duration::from_millis(300)))
        .expect("timed");
    socket
}

/// Binds `socket` at the relay `at` to the end of a pair `token` names, waiting for the answer.
fn bind_at(socket: &UdpSocket, at: SocketAddr, token: &Token) {
    let mut datagram = Vec::new();
    bind(token, &mut datagram);
    socket.send_to(&datagram, at).expect("sent");
    let mut answer = [0; 128];
    let (size, _) = socket.recv_from(&mut answer).expect("answered");
    assert_eq!(
        bound(&answer[..size]),
        Some(socket.local_addr().expect("an address"))
    );
}

/// Has `relay` pair a host of `room` with a joiner, as a host's run and a joiner's do: the joiner's
/// end, then the host's.
fn pair(relay: &Relay, room: Room) -> (Paired, Paired) {
    let at = relay.address();
    let mut hosting = Hosting::start(at, room).expect("hosting");
    let joining = thread::spawn(move || join(at, room));
    let ticket = loop {
        if let Some(ticket) = hosting.joiner().expect("the relay there") {
            break ticket;
        }
    };
    let host = take(at, &ticket).expect("taken");
    match joining.join().expect("joined").expect("paired") {
        Joined::Paired(joiner) => (joiner, host),
        Joined::Unhosted => panic!("the room hosted"),
    }
}

/// Reads `stream` to its end, within 10 s.
fn read_to_end(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut read = Vec::new();
    stream.read_to_end(&mut read).map(|_| read)
}

fn refusal(error: &io::Error) -> String {
    assert_eq!(error.kind(), ErrorKind::ConnectionRefused, "{error}");
    error.to_string()
}

/// A host and a joiner of one room are paired, each told a token of its own, and what each writes
/// reaches the other, in order, the bytes after `paired` among them; one end closing its sending
/// side ends what the other reads, after what came, and the other can still write back.
#[test]
fn a_host_and_a_joiner_are_paired_and_their_bytes_cross() {
    let relay = relay(most(16));
    let (mut joiner, mut host) = pair(&relay, ROOM);
    assert_ne!(joiner.token, host.token);
    // At once, as a joiner's handshake follows its `paired`.
    joiner
        .stream
        .write_all(b"from the joiner")
        .expect("written");
    host.stream.write_all(b"from the host").expect("written");
    let mut came = [0; 15];
    host.stream.read_exact(&mut came).expect("read");
    assert_eq!(&came, b"from the joiner");
    let mut came = [0; 13];
    joiner.stream.read_exact(&mut came).expect("read");
    assert_eq!(&came, b"from the host");

    host.stream.write_all(b"last").expect("written");
    host.stream.shutdown(Shutdown::Write).expect("shut");
    assert_eq!(read_to_end(&mut joiner.stream).expect("read"), b"last");
    joiner.stream.write_all(b"after").expect("written");
    joiner.stream.shutdown(Shutdown::Write).expect("shut");
    assert_eq!(read_to_end(&mut host.stream).expect("read"), b"after");
    relay.stop();
}

/// A joiner of a room no host holds finds none, and is paired once a host registers it; a second
/// host of a room held is refused, and takes it once the first has gone.
#[test]
fn a_room_is_its_first_host_s_alone_and_a_joiner_finds_none_without_one() {
    let relay = relay(most(16));
    let at = relay.address();
    assert!(matches!(join(at, ROOM), Ok(Joined::Unhosted)));
    let first = Hosting::start(at, ROOM).expect("hosting");
    let second = Hosting::start(at, ROOM).err().expect("refused");
    assert!(
        refusal(&second).contains("another host holds the room"),
        "{second}"
    );
    drop(first);
    let start = Instant::now();
    let second = loop {
        match Hosting::start(at, ROOM) {
            Ok(second) => break second,
            Err(error) => {
                refusal(&error);
                assert!(start.elapsed() < Duration::from_secs(5), "never freed");
                thread::sleep(Duration::from_millis(10));
            }
        }
    };
    drop(second);
    // Another room's host is its own.
    let (mut joiner, mut host) = pair(&relay, Room([8; 16]));
    joiner.stream.write_all(b"1").expect("written");
    host.stream.read_exact(&mut [0]).expect("read");
    // Nothing kept of the rooms and the pair once their connections have closed.
    drop((joiner, host));
    until_empty(&relay);
    relay.stop();
}

/// A take of a ticket the relay didn't give is refused, as is one of a joiner its host didn't take
/// in time, which the relay refuses; and past the most connections, a connection is refused, the
/// relay full.
#[test]
fn a_relay_refuses_tickets_it_didn_t_give_joiners_left_waiting_and_connections_past_its_most() {
    let relay = serve_within("127.0.0.1:0", most(3), ANSWER, WAITED).expect("a relay");
    let at = relay.address();
    let error = take(at, &Ticket([1; 32])).err().expect("refused");
    assert!(refusal(&error).contains("no joiner waits"), "{error}");

    let mut hosting = Hosting::start(at, ROOM).expect("hosting");
    let joining = thread::spawn(move || join(at, ROOM));
    let ticket = loop {
        if let Some(ticket) = hosting.joiner().expect("the relay there") {
            break ticket;
        }
    };
    let start = Instant::now();
    let error = joining.join().expect("joined").err().expect("refused");
    assert!(
        refusal(&error).contains("didn't take it within 1 s"),
        "{error}"
    );
    let waited = start.elapsed();
    assert!(
        waited >= WAITED * 4 / 5 && waited < WAITED + Duration::from_secs(4),
        "{waited:?}"
    );
    let error = take(at, &ticket).err().expect("refused");
    assert!(refusal(&error).contains("no joiner waits"), "{error}");

    // The host's one connection and two more held, the next is one past the most.
    let held = [open(at).expect("open"), open(at).expect("open")];
    let start = Instant::now();
    loop {
        // Until the relay has seen the takes' and the joiner's connections close.
        let error = take(at, &Ticket([1; 32])).err().expect("refused");
        if refusal(&error).contains("full") {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "never full: {error}"
        );
        thread::sleep(Duration::from_millis(10));
    }
    drop((held, hosting));
    relay.stop();
}

/// A connection whose first line doesn't come in time, is too long, isn't one, or speaks another
/// version of the protocol is closed, answered with why if it speaks the protocol.
#[test]
fn a_connection_whose_first_line_isn_t_one_is_closed() {
    let relay = serve_within("127.0.0.1:0", most(16), WAITED, TAKE).expect("a relay");
    let at = relay.address();
    let send = |text: &[u8]| {
        let mut stream = TcpStream::connect(at).expect("connected");
        stream.write_all(text).expect("written");
        read_to_end(&mut stream)
    };
    // Closed, or reset, as what wasn't read may have it: not left open.
    let ended = |read: io::Result<Vec<u8>>| match read {
        Ok(read) => assert!(read.is_empty(), "{read:?}"),
        Err(error) => assert_ne!(error.kind(), ErrorKind::WouldBlock, "{error}"),
    };
    ended(send(b"GET / HTTP/1.1\n"));
    ended(send(&[b'g'; MAX_LINE + 10]));
    let answer = send(b"gg-relay 2 host 07070707070707070707070707070707\n").expect("read");
    let answer = String::from_utf8(answer).expect("text");
    assert_eq!(
        answer,
        "refused it speaks gg-relay 2, and the relay gg-relay 1\n"
    );
    let answer = send(b"gg-relay 1 dance\n").expect("read");
    assert_eq!(answer, b"refused its first line isn't one\n");
    let answer = send(b"gg-relay 1 host 0707\n").expect("read");
    assert_eq!(answer, b"refused a room is 32 hex digits\n");

    let start = Instant::now();
    ended(send(b""));
    let waited = start.elapsed();
    assert!(
        waited >= WAITED * 4 / 5 && waited < WAITED + Duration::from_secs(4),
        "{waited:?}"
    );
    relay.stop();
}

/// A bind is answered with the address it came from; datagrams between a pair's two bound sockets
/// cross each way, the relay's own aside; one from a socket not bound, or once the pair has ended,
/// goes nowhere; a bind with a token the relay didn't give gets no answer.
#[test]
fn a_pair_s_bound_sockets_datagrams_cross_and_no_one_else_s() {
    let relay = relay(most(16));
    let at = relay.address();
    let (joiner, host) = pair(&relay, ROOM);
    let (ours, theirs, stranger) = (socket(), socket(), socket());
    let mut buffer = [0; 2048];
    let mut came = |socket: &UdpSocket| {
        socket
            .recv_from(&mut buffer)
            .ok()
            .map(|(size, from)| (buffer[..size].to_vec(), from))
    };
    let mut datagram = Vec::new();
    for (socket, token) in [(&ours, &joiner.token), (&theirs, &host.token)] {
        bind(token, &mut datagram);
        socket.send_to(&datagram, at).expect("sent");
        let (answer, from) = came(socket).expect("answered");
        assert_eq!(from, at);
        assert_eq!(
            bound(&answer),
            Some(socket.local_addr().expect("an address"))
        );
    }
    bind(&Token([3; 32]), &mut datagram);
    stranger.send_to(&datagram, at).expect("sent");
    assert_eq!(came(&stranger), None);

    ours.send_to(b"\x80 across", at).expect("sent");
    assert_eq!(came(&theirs), Some((b"\x80 across".to_vec(), at)));
    theirs.send_to(b"\x81 back", at).expect("sent");
    assert_eq!(came(&ours), Some((b"\x81 back".to_vec(), at)));
    ours.send_to(b"gg-relay bound 1.2.3.4:5", at).expect("sent");
    assert_eq!(came(&theirs), None);
    stranger.send_to(b"\x80 in", at).expect("sent");
    assert_eq!((came(&ours), came(&theirs)), (None, None));
    assert_eq!(relay.forwarded(), 2);

    joiner.stream.shutdown(Shutdown::Write).expect("shut");
    host.stream.shutdown(Shutdown::Write).expect("shut");
    let start = Instant::now();
    loop {
        ours.send_to(b"\x80 after", at).expect("sent");
        if came(&theirs).is_none() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the pair never ended"
        );
    }
    // Nothing of the pair kept: its connections, its tokens, its sockets.
    drop((joiner, host));
    until_empty(&relay);
    relay.stop();
}

/// Past the most connections from one address, a connection is refused, saying why, and once one
/// of them closes, another is taken; the address's count goes once they have all gone.
#[test]
fn a_relay_refuses_connections_past_its_most_from_one_address() {
    let relay = relay(Limits {
        per_address: 3,
        ..most(16)
    });
    let at = relay.address();
    let mut held = vec![
        open(at).expect("open"),
        open(at).expect("open"),
        open(at).expect("open"),
    ];
    // A take is answered once its connection is taken, and otherwise refused before.
    let error = take(at, &Ticket([1; 32])).err().expect("refused");
    assert!(
        refusal(&error).contains("its address holds 3 connections, the most the relay holds"),
        "{error}"
    );
    held.pop();
    let start = Instant::now();
    loop {
        let error = take(at, &Ticket([1; 32])).err().expect("refused");
        if refusal(&error).contains("no joiner waits") {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "never taken: {error}"
        );
        thread::sleep(Duration::from_millis(10));
    }
    drop(held);
    until_empty(&relay);
    relay.stop();
}

/// What a connection counts as toward the most from one address: an IPv4 address itself, or as
/// IPv6 maps it; an IPv6 address by its /64.
#[test]
fn an_address_counts_as_itself_or_its_64() {
    let ip = |text: &str| text.parse::<IpAddr>().expect("an address");
    assert_eq!(key(ip("203.0.113.7")), ip("203.0.113.7"));
    assert_eq!(key(ip("::ffff:203.0.113.7")), ip("203.0.113.7"));
    assert_eq!(key(ip("2001:db8:1:2:3:4:5:6")), ip("2001:db8:1:2::"));
    assert_eq!(key(ip("2001:db8:1:2:ffff::1")), ip("2001:db8:1:2::"));
    assert_ne!(key(ip("2001:db8:1:3::1")), ip("2001:db8:1:2::"));
}

/// An end that sends datagrams faster than its rate has its burst forwarded, and then the rate's,
/// the rest dropped; and the other end's still go, as its rate is its own. Likewise for their
/// bytes.
#[test]
fn a_relay_drops_an_end_s_datagrams_past_its_rates() {
    let rate = |per_second, burst| Rate { per_second, burst };
    let lots = rate(1 << 20, 1 << 20);
    for (limits, size, burst, per_second) in [
        (rate(100, 50), lots, 1, 50, 100),
        (lots, rate(20_000, 40_000), 1000, 40, 20),
    ]
    .map(|(datagrams, datagram_bytes, size, burst, per_second)| {
        let limits = Limits {
            datagrams,
            datagram_bytes,
            ..most(16)
        };
        (limits, size, burst, per_second)
    }) {
        let relay = relay(limits);
        let at = relay.address();
        let (joiner, host) = pair(&relay, ROOM);
        let (ours, theirs) = (socket(), socket());
        bind_at(&ours, at, &joiner.token);
        bind_at(&theirs, at, &host.token);
        // Paced, so that none are lost on the way to the relay, a few a millisecond.
        let datagram = vec![0x80; size];
        let start = Instant::now();
        for sent in 0..500 {
            ours.send_to(&datagram, at).expect("sent");
            if sent % 10 == 9 {
                thread::sleep(Duration::from_millis(1));
            }
        }
        // Once the relay has taken the last, all forwarded before the bound is reckoned.
        thread::sleep(Duration::from_millis(50));
        let at_most = burst as f64 + per_second as f64 * start.elapsed().as_secs_f64() + 1.0;
        let forwarded = relay.forwarded();
        assert!(
            forwarded >= burst && forwarded as f64 <= at_most,
            "{forwarded} forwarded of 500, where the burst is {burst} and at most {at_most} pass"
        );
        theirs.send_to(&datagram, at).expect("sent");
        let mut came = vec![0; 2048];
        let (size_came, _) = ours.recv_from(&mut came).expect("the other end's came");
        assert_eq!(size_came, size);
        drop((joiner, host));
        relay.stop();
    }
}

/// An end that sends its stream faster than its rate has it all arrive, in order, held back to the
/// rate past its burst.
#[test]
fn a_relay_holds_an_end_s_stream_to_its_rate() {
    let stream = Rate {
        per_second: 64 << 10,
        burst: 16 << 10,
    };
    let relay = relay(Limits { stream, ..most(16) });
    let (mut joiner, mut host) = pair(&relay, ROOM);
    let sent: Vec<u8> = (0..128 << 10).map(|i| (i % 251) as u8).collect();
    let start = Instant::now();
    let writing = {
        let sent = sent.clone();
        thread::spawn(move || joiner.stream.write_all(&sent).map(|()| joiner))
    };
    let mut came = vec![0; sent.len()];
    host.stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timed");
    host.stream.read_exact(&mut came).expect("read");
    let took = start.elapsed();
    assert!(came == sent, "the bytes came otherwise");
    // 112 KiB past the burst, at 64 KiB a second.
    assert!(
        took >= Duration::from_millis(1740) && took < Duration::from_secs(5),
        "{took:?}"
    );
    drop(writing.join().expect("written").expect("written"));
    relay.stop();

    let error = serve(
        "127.0.0.1:0",
        Limits {
            stream: Rate {
                per_second: 0,
                burst: 1,
            },
            ..Limits::default()
        },
    )
    .err()
    .expect("refused");
    assert_eq!(error.kind(), ErrorKind::InvalidInput, "{error}");
}

/// A relay stopped closes the connections it held, and its port, which another relay can then
/// take.
#[test]
fn a_relay_stopped_closes_its_connections_and_its_port() {
    let relay = relay(most(16));
    let at = relay.address();
    let mut hosting = Hosting::start(at, ROOM).expect("hosting");
    relay.stop();
    let start = Instant::now();
    while hosting.joiner().is_ok() {
        assert!(start.elapsed() < Duration::from_secs(5), "never closed");
    }
    let again = serve(&at.to_string(), most(16)).expect("the same port");
    Hosting::start(at, ROOM).expect("hosting again");
    again.stop();
}

/// A relay's arguments: where it listens, the most connections it holds, and from one address, and
/// where it logs.
#[test]
fn a_relay_s_arguments() {
    let parsed = |args: &[&str]| parse(args.iter().map(OsString::from));
    let default = Options {
        listen: LISTEN.to_owned(),
        limits: Limits::default(),
        logs: None,
    };
    assert_eq!(parsed(&[]), Ok(Some(default)));
    let given = parsed(&[
        "--listen",
        "127.0.0.1:7801",
        "--most=2",
        "--per-address",
        "3",
        "--logs",
        "/var/log/gg-relay",
    ]);
    let expected = Options {
        listen: "127.0.0.1:7801".to_owned(),
        limits: Limits {
            most: 2,
            per_address: 3,
            ..Limits::default()
        },
        logs: Some(PathBuf::from("/var/log/gg-relay")),
    };
    assert_eq!(given, Ok(Some(expected)));
    assert_eq!(parsed(&["--help"]), Ok(None));
    for bad in [
        &["--most", "0"][..],
        &["--most", "many"],
        &["--per-address", "0"],
        &["--per-address", "some"],
        &["--logs", ""],
        &["--logs"],
        &["--listen"],
        &["--port", "1"],
    ] {
        assert!(parsed(bad).is_err(), "{bad:?}");
    }
}
