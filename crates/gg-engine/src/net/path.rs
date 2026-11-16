//! Which way a link through a relay sends its packets: through the relay, direct to the other end,
//! or both, as its packets show the ways work.
//!
//! Each end tells the other on the stream where its socket is (`at`): as the relay saw it, which
//! is as its router maps it, and as it's bound, for two ends behind one router. Given the other's,
//! a link sends each packet through the relay and to each of the other's addresses, for [`PUNCH`].
//! Each end's sending opens its own router for the other's packets, as UDP hole punching does
//! (Ford, Srisuresh and Kegel, "Peer-to-Peer Communication Across Network Address Translators",
//! USENIX 2005: it got through 82% of the NATs they tested, TCP's 64%, which is why the stream
//! stays on the relay).
//!
//! Each packet says, in a byte sealed inside it, whether its link has taken a packet that came
//! direct within [`QUIET`]. A link sends direct alone, to where the latest direct packet came
//! from, while it has and the other's latest packet says it has too: a way that works one way
//! alone is never taken, and the ends settle on the same way. A way direct that stops falls back
//! to the relay within [`QUIET`] and probes again for [`PUNCH`]. Routers that map each destination
//! their own way leave the packets on the relay.
//!
//! A state machine without a clock or I/O: a link gives it each instant.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// How long a link probes the other's addresses, sending both ways, once given them or after a way
/// direct stops.
pub(super) const PUNCH: Duration = Duration::from_secs(5);

/// How long since a packet last came direct a link still counts the way direct as working.
pub(super) const QUIET: Duration = Duration::from_millis(250);

#[derive(Debug, PartialEq)]
pub(super) struct Ways {
    pub(super) relay: bool,
    pub(super) direct: Vec<SocketAddr>,
}

/// A change of way, which a link logs.
#[derive(Debug, PartialEq)]
pub(super) enum Change {
    /// Packets go direct alone, to this address.
    Direct(SocketAddr),
    /// They go back to the relay: the way direct stopped.
    Stopped,
    /// A probe's time passed without a way direct both ways.
    Unpunched,
}

/// A link's way.
#[derive(Default)]
pub(super) struct Path {
    theirs: Vec<SocketAddr>,
    pending: bool,
    probing: Option<Instant>,
    heard: Option<(SocketAddr, Instant)>,
    /// Whether the other's latest packet said it has had one direct within [`QUIET`].
    confirmed: bool,
    direct: bool,
}

impl Path {
    /// Takes the other's addresses `theirs` for a socket bound at `ours`, skipping those that could
    /// reach another than the other end: another family's, unspecified, multicast, the loopback's
    /// (unless `ours` is too). A probe starts as the next packet goes.
    pub(super) fn at(&mut self, theirs: [SocketAddr; 2], ours: SocketAddr) {
        let reachable = |at: &SocketAddr| {
            let ip = at.ip();
            ip.is_ipv4() == ours.is_ipv4()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && at.port() != 0
                && (!ip.is_loopback() || ours.ip().is_loopback())
                && !matches!(ip, IpAddr::V4(ip) if ip.is_broadcast())
        };
        self.theirs.clear();
        for at in theirs.into_iter().filter(reachable) {
            if !self.theirs.contains(&at) {
                self.theirs.push(at);
            }
        }
        self.pending = !self.theirs.is_empty();
    }

    /// Takes a packet come at `now`, direct from `from` or through the relay (`None`); `confirms`
    /// whether it says the other has had one direct within [`QUIET`].
    pub(super) fn came(&mut self, from: Option<SocketAddr>, confirms: bool, now: Instant) {
        if let Some(from) = from {
            self.heard = Some((from, now));
        }
        self.confirmed = confirms;
    }

    /// Whether a packet came direct within [`QUIET`] of `now`: what this end's packets say.
    pub(super) fn hears_direct(&self, now: Instant) -> bool {
        self.heard
            .is_some_and(|(_, came)| now.saturating_duration_since(came) < QUIET)
    }

    pub(super) fn step(&mut self, now: Instant) -> (Ways, Option<Change>) {
        if std::mem::take(&mut self.pending) {
            self.probing = Some(now);
        }
        let hears = self.hears_direct(now);
        let mut change = None;
        if hears && self.confirmed {
            let (to, _) = self.heard.expect("a packet came direct");
            if !std::mem::replace(&mut self.direct, true) {
                change = Some(Change::Direct(to));
            }
            self.probing = None;
            let ways = Ways {
                relay: false,
                direct: vec![to],
            };
            return (ways, change);
        }
        if std::mem::take(&mut self.direct) {
            change = Some(Change::Stopped);
            self.probing = Some(now);
        }
        let probing = self
            .probing
            .is_some_and(|start| now.saturating_duration_since(start) < PUNCH);
        if !probing && self.probing.take().is_some() {
            change = Some(Change::Unpunched);
        }
        let mut direct = Vec::new();
        if probing {
            direct.extend_from_slice(&self.theirs);
            // Where the other's came from: a router mapping each destination its own way may make
            // it another address than either given.
            if let Some((from, _)) = self.heard.filter(|_| hears)
                && !direct.contains(&from)
            {
                direct.push(from);
            }
        }
        let ways = Ways {
            relay: true,
            direct,
        };
        (ways, change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> SocketAddr {
        text.parse().expect("an address")
    }

    const MS: Duration = Duration::from_millis(1);

    /// Before the other's addresses come, packets go through the relay alone. Given them, each
    /// also goes direct to each, for [`PUNCH`] from the first sent. Past it, with none come
    /// direct, through the relay alone again, said once.
    #[test]
    fn a_link_probes_the_other_s_addresses_for_a_while() {
        let start = Instant::now();
        let mut path = Path::default();
        let relayed = Ways {
            relay: true,
            direct: Vec::new(),
        };
        assert_eq!(path.step(start), (relayed, None));
        let (mapped, bound) = (at("203.0.113.7:40000"), at("192.168.1.5:50123"));
        path.at([mapped, bound], at("10.0.0.2:50000"));
        let later = start + Duration::from_secs(30);
        let probing = Ways {
            relay: true,
            direct: vec![mapped, bound],
        };
        assert_eq!(path.step(later), (probing, None));
        let probing = path.step(later + PUNCH - MS).0;
        assert_eq!(probing.direct, [mapped, bound]);
        let (ways, change) = path.step(later + PUNCH);
        assert_eq!((ways.relay, ways.direct), (true, Vec::new()));
        assert_eq!(change, Some(Change::Unpunched));
        assert_eq!(path.step(later + PUNCH + MS).1, None);
    }

    /// A packet come direct, while the other's don't say it has had one: both ways still. Once
    /// they do: direct alone, to where it came from, said once. Back to the relay, probing again,
    /// once none has come direct for [`QUIET`] or the other's say it hasn't.
    #[test]
    fn a_link_goes_direct_once_both_ends_hear_each_other_so() {
        let start = Instant::now();
        let mut path = Path::default();
        let (mapped, bound) = (at("203.0.113.7:40000"), at("192.168.1.5:50123"));
        path.at([mapped, bound], at("10.0.0.2:50000"));
        path.step(start);
        // From another port than given, as a router mapping each destination its own way does.
        let from = at("203.0.113.7:40001");
        path.came(Some(from), false, start + MS);
        assert!(path.hears_direct(start + MS));
        let (ways, change) = path.step(start + 2 * MS);
        assert_eq!(
            (ways.relay, ways.direct, change),
            (true, vec![mapped, bound, from], None)
        );
        path.came(None, true, start + 3 * MS);
        let direct = Ways {
            relay: false,
            direct: vec![from],
        };
        let (ways, change) = path.step(start + 4 * MS);
        assert_eq!((ways, change), (direct, Some(Change::Direct(from))));
        assert_eq!(path.step(start + 5 * MS).1, None);

        let quiet = start + MS + QUIET;
        assert!(!path.hears_direct(quiet));
        let (ways, change) = path.step(quiet);
        assert_eq!((ways.relay, change), (true, Some(Change::Stopped)));
        assert_eq!(ways.direct, [mapped, bound]);
        path.came(Some(from), true, quiet + MS);
        assert_eq!(path.step(quiet + MS).1, Some(Change::Direct(from)));
        path.came(Some(from), false, quiet + 2 * MS);
        let (ways, change) = path.step(quiet + 2 * MS);
        assert_eq!((ways.relay, change), (true, Some(Change::Stopped)));
    }

    /// A way that works one way alone (this end hears the other direct, the other never says it
    /// hears this end) is never taken alone.
    #[test]
    fn a_way_that_works_one_way_alone_is_never_taken() {
        let start = Instant::now();
        let mut path = Path::default();
        let mapped = at("203.0.113.7:40000");
        path.at([mapped, mapped], at("10.0.0.2:50000"));
        for ms in 0..PUNCH.as_millis() as u32 + 100 {
            let now = start + ms * MS;
            path.came(Some(mapped), false, now);
            let (ways, change) = path.step(now);
            assert!(ways.relay, "at {ms} ms");
            assert!(!matches!(change, Some(Change::Direct(_))), "at {ms} ms");
        }
    }

    /// Addresses that could reach another than the other end are skipped.
    #[test]
    fn addresses_that_could_reach_another_are_skipped() {
        let start = Instant::now();
        let ours = at("192.168.1.5:50000");
        let mut path = Path::default();
        for skipped in [
            "[2001:db8::1]:40000",
            "0.0.0.0:40000",
            "127.0.0.1:40000",
            "255.255.255.255:9",
        ] {
            path.at([at(skipped), at(skipped)], ours);
            assert_eq!(path.step(start).0.direct, Vec::new(), "{skipped}");
        }
        path.at(
            [at("127.0.0.1:40000"), at("127.0.0.1:40000")],
            at("127.0.0.1:50000"),
        );
        assert_eq!(path.step(start).0.direct, [at("127.0.0.1:40000")]);
    }
}
