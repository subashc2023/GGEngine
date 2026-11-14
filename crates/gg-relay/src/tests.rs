use std::net::{Shutdown, UdpSocket};
use std::thread;

use super::relay::serve_within;
use super::*;

const ROOM: Room = Room([7; 16]);

/// How long a relay waits for a first line or a host's take in the tests that wait it out, in place
/// of [`ANSWER`] or [`TAKE`].
const WAITED: Duration = Duration::from_secs(1);

fn relay(most: usize) -> Relay {
    serve("127.0.0.1:0", most).expect("a relay")
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
    let relay = relay(16);
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
    let relay = relay(16);
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
    let start = Instant::now();
    while relay.holds() != [0; 6] {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            relay.holds()
        );
        thread::sleep(Duration::from_millis(10));
    }
    relay.stop();
}

/// A take of a ticket the relay didn't give is refused, as is one of a joiner its host didn't take
/// in time, which the relay refuses; and past the most connections, a connection is refused, the
/// relay full.
#[test]
fn a_relay_refuses_tickets_it_didn_t_give_joiners_left_waiting_and_connections_past_its_most() {
    let relay = serve_within("127.0.0.1:0", 3, ANSWER, WAITED).expect("a relay");
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
    let relay = serve_within("127.0.0.1:0", 16, WAITED, TAKE).expect("a relay");
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
    let relay = relay(16);
    let at = relay.address();
    let (joiner, host) = pair(&relay, ROOM);
    let socket = || {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bound");
        socket
            .set_read_timeout(Some(Duration::from_millis(300)))
            .expect("timed");
        socket
    };
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
    let start = Instant::now();
    while relay.holds() != [0; 6] {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            relay.holds()
        );
        thread::sleep(Duration::from_millis(10));
    }
    relay.stop();
}

/// A relay stopped closes the connections it held, and its port, which another relay can then
/// take.
#[test]
fn a_relay_stopped_closes_its_connections_and_its_port() {
    let relay = relay(16);
    let at = relay.address();
    let mut hosting = Hosting::start(at, ROOM).expect("hosting");
    relay.stop();
    let start = Instant::now();
    while hosting.joiner().is_ok() {
        assert!(start.elapsed() < Duration::from_secs(5), "never closed");
    }
    let again = serve(&at.to_string(), 16).expect("the same port");
    Hosting::start(at, ROOM).expect("hosting again");
    again.stop();
}

/// A relay's arguments: where it listens and the most connections it holds.
#[test]
fn a_relay_s_arguments() {
    let parsed = |args: &[&str]| parse(args.iter().map(OsString::from));
    assert_eq!(parsed(&[]), Ok(Some((LISTEN.to_owned(), MOST))));
    let given = parsed(&["--listen", "127.0.0.1:7801", "--most=2"]);
    assert_eq!(given, Ok(Some(("127.0.0.1:7801".to_owned(), 2))));
    assert_eq!(parsed(&["--help"]), Ok(None));
    for bad in [
        &["--most", "0"][..],
        &["--most", "many"],
        &["--listen"],
        &["--port", "1"],
    ] {
        assert!(parsed(bad).is_err(), "{bad:?}");
    }
}
