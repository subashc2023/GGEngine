//! What keeps a session to the runs given its code
//! ([Networking](../../../../ARCHITECTURE.md#networking)): the code; the Noise `NNpsk0`
//! handshake, through `snow`, which opens each connection, is keyed by the code and makes the
//! link's keys; and the stream's frames and the packets, sealed under those keys. Stream nonces
//! count from 0 each way and packets' from [`PACKETS`], so no two share one under a key.
//!
//! A frame is its length, 2 bytes big-endian, and its sealed text, at most 65,535 bytes together,
//! as a Noise message. A packet is its nonce, 8 bytes big-endian, and its sealed text, [`SEALED`]
//! bytes more than the text. A packet that doesn't open, or whose nonce came before, is dropped
//! ([`Replays`]); a frame that doesn't open ends its stream.

use std::fmt::{self, Write as _};
use std::io::{self, BufRead, ErrorKind, Read, Write};
use std::str::FromStr;
use std::sync::Arc;

use gg_relay::Room;
use snow::params::HashChoice;
use snow::resolvers::{CryptoResolver, DefaultResolver};
use snow::{Builder, StatelessTransportState};

/// Noise's NN, each end's key made for the connection alone, the code's key mixed in before the
/// first message (`psk0`): the end that took the connection refuses a run without the code at the
/// first message, before any work of its own. X25519, ChaCha20-Poly1305 and BLAKE2s are fast
/// without the CPU's AES instructions.
const PATTERN: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";

/// What both ends' handshakes are bound to, so a later version of these formats changes it.
const PROLOGUE: &[u8] = b"gg-engine session 1";

/// The keys of the code's HMACs: its own (Noise's pre-shared key) and its room's at a relay. They
/// differ, so the code's key isn't found from the room.
const CODE_KEY: &[u8] = b"gg-engine session code";
const ROOM_KEY: &[u8] = b"gg-engine relay room";

/// Bytes Noise's tag takes, and the most a Noise message may have, tag included.
const TAG: usize = 16;
const MOST: usize = 65535;

/// The first nonce of a link's packets each way: apart from the stream's, which counts from 0 and
/// never reaches it.
pub(super) const PACKETS: u64 = 1 << 63;

/// The bytes sealing adds to a packet's text: nonce and tag.
pub(super) const SEALED: usize = 8 + TAG;

/// Crockford's base 32: the digits and the letters but I, L, O and U, which read as others or
/// spell words.
const SYMBOLS: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A session's code, given to each of its runs, which keys its links: 80 random bits, written as
/// 16 of [`SYMBOLS`] in fours, as `7KQ2-9XMB-4TZD-P0WH`. Its `Debug` writes none of it, as it's the
/// session's key.
///
/// 80 bits suffice: a first message recorded on the path lets one test codes against it
/// offline, a microsecond each, so 2⁸⁰ of them take some 10¹⁴ s at 10¹⁰ a second. Each
/// handshake's keys also come from both ends' keys made for it, so a code found later opens no
/// earlier link.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Code([u8; 10]);

impl Code {
    /// A code from the OS's randomness, as Noise's own keys are.
    ///
    /// Fails if the OS gives none.
    pub(crate) fn make() -> io::Result<Self> {
        let mut bytes = [0; 10];
        let mut random = DefaultResolver
            .resolve_rng()
            .ok_or_else(|| io::Error::other("no source of random numbers"))?;
        random
            .try_fill_bytes(&mut bytes)
            .map_err(|error| io::Error::other(format!("the OS gave no random numbers: {error}")))?;
        Ok(Self(bytes))
    }

    fn key(&self) -> [u8; 32] {
        self.hmac(CODE_KEY)
    }

    /// The session's room at a relay, where its runs find each other: the first 16 bytes of the
    /// code's HMAC-BLAKE2s under [`ROOM_KEY`]. A relay, and anyone on the path to it, sees the
    /// room and can't find the code or its key (an HMAC under another key) from it.
    pub(crate) fn room(&self) -> Room {
        let hmac = self.hmac(ROOM_KEY);
        Room(hmac[..16].try_into().expect("32 bytes"))
    }

    fn hmac(&self, key: &[u8]) -> [u8; 32] {
        let mut hash = DefaultResolver
            .resolve_hash(&HashChoice::Blake2s)
            .expect("snow's resolver has BLAKE2s, as its `use-blake2` feature is on");
        let mut hmac = [0; 32];
        hash.hmac(key, &self.0, &mut hmac);
        hmac
    }
}

/// 16 symbols, any case; `I` and `L` read as `1`, `O` as `0`; hyphens and spaces skipped.
impl FromStr for Code {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let (mut bits, mut symbols) = (0u128, 0);
        for symbol in text.chars() {
            let value = match symbol.to_ascii_uppercase() {
                '-' | ' ' => continue,
                'I' | 'L' => 1,
                'O' => 0,
                upper => {
                    (SYMBOLS.iter().position(|&of| char::from(of) == upper)).ok_or_else(|| {
                        format!("`{symbol}` isn't one of its symbols, the digits and letters but U")
                    })?
                }
            };
            symbols += 1;
            bits = bits << 5 | value as u128;
        }
        if symbols != 16 {
            return Err(format!("it has {symbols} symbols, and a code 16"));
        }
        let bytes = bits.to_be_bytes();
        Ok(Self(bytes[6..].try_into().expect("16 bytes less 6")))
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut bytes = [0; 16];
        bytes[6..].copy_from_slice(&self.0);
        let bits = u128::from_be_bytes(bytes);
        for at in 0..16 {
            if at > 0 && at % 4 == 0 {
                f.write_char('-')?;
            }
            let value = (bits >> (75 - 5 * at)) & 31;
            f.write_char(char::from(SYMBOLS[value as usize]))?;
        }
        Ok(())
    }
}

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Code(..)")
    }
}

/// A link's keys, one per direction, made by its handshake. Usable from any thread, each message
/// under the nonce it's given.
pub(super) type Keys = StatelessTransportState;

/// Runs the handshake on `stream` keyed by `code`, as the end that made the connection or, if
/// `accepted`, took it; returns the link's keys. The maker writes the first message, which the
/// other can't read without the code; the other answers. Each message is a frame. Reads wait as
/// long as the stream's timeout.
///
/// Fails with:
/// - `InvalidData` at a first message that doesn't open, from a run of another session or what
///   isn't a run, or an answer that doesn't;
/// - `UnexpectedEof` where the other end closed the connection, as one that refuses this end does;
/// - the stream's errors.
pub(super) fn handshake<S: Read + Write>(
    stream: &mut S,
    code: &Code,
    accepted: bool,
) -> io::Result<Keys> {
    let key = code.key();
    let builder = Builder::new(PATTERN.parse().expect("a pattern snow knows"))
        .psk(0, &key)
        .and_then(|builder| builder.prologue(PROLOGUE))
        .map_err(io::Error::other)?;
    let made = if accepted {
        builder.build_responder()
    } else {
        builder.build_initiator()
    };
    let mut state = made.map_err(io::Error::other)?;
    // Each message, a key and an empty payload's tag, 48 bytes.
    let (mut message, mut payload) = ([0; 64], [0; 64]);
    let closed = |why: &str| io::Error::new(ErrorKind::UnexpectedEof, why.to_owned());
    if accepted {
        let size =
            read_frame(stream, &mut message)?.ok_or_else(|| closed("it closed the connection"))?;
        state
            .read_message(&message[..size], &mut payload)
            .map_err(|_| {
                io::Error::new(ErrorKind::InvalidData, "it doesn't hold the session's code")
            })?;
        let size = state
            .write_message(&[], &mut message)
            .map_err(io::Error::other)?;
        write_frame(stream, &message[..size])?;
    } else {
        let size = state
            .write_message(&[], &mut message)
            .map_err(io::Error::other)?;
        write_frame(stream, &message[..size])?;
        let size = read_frame(stream, &mut message)?.ok_or_else(|| {
            closed("it closed the connection, as it does a run without the session's code")
        })?;
        state
            .read_message(&message[..size], &mut payload)
            .map_err(|_| io::Error::new(ErrorKind::InvalidData, "its answer doesn't open"))?;
    }
    state
        .into_stateless_transport_mode()
        .map_err(io::Error::other)
}

/// Reads a frame into `frame`, returning its length; `None` once the stream ends between frames.
///
/// Fails with `UnexpectedEof` where it ends within one, and `InvalidData` for one longer than
/// `frame`.
fn read_frame(reader: &mut impl Read, frame: &mut [u8]) -> io::Result<Option<usize>> {
    let mut length = [0; 2];
    loop {
        match reader.read(&mut length[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    reader.read_exact(&mut length[1..])?;
    let size = usize::from(u16::from_be_bytes(length));
    let read = frame.get_mut(..size).ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("a frame of {size} bytes, past the most"),
        )
    })?;
    reader.read_exact(read)?;
    Ok(Some(size))
}

/// Writes `message` as a frame in one write, so Nagle's algorithm doesn't hold it until the length
/// is acknowledged.
fn write_frame(writer: &mut impl Write, message: &[u8]) -> io::Result<()> {
    let length = u16::try_from(message.len()).expect("a message is at most 65,535 bytes");
    writer.write_all(&[&length.to_be_bytes()[..], message].concat())
}

/// Writes texts to a stream in sealed frames, nonces from 0, for the other's [`Opened`].
pub(super) struct Sealer {
    keys: Arc<Keys>,
    nonce: u64,
    frame: Vec<u8>,
}

impl Sealer {
    pub(super) fn new(keys: Arc<Keys>) -> Self {
        Self {
            keys,
            nonce: 0,
            frame: vec![0; 2 + MOST],
        }
    }

    pub(super) fn write(&mut self, stream: &mut impl Write, text: &[u8]) -> io::Result<()> {
        for part in text.chunks(MOST - TAG) {
            let size = self
                .keys
                .write_message(self.nonce, part, &mut self.frame[2..])
                .map_err(io::Error::other)?;
            self.nonce += 1;
            let length = u16::try_from(size).expect("a message is at most 65,535 bytes");
            self.frame[..2].copy_from_slice(&length.to_be_bytes());
            stream.write_all(&self.frame[..2 + size])?;
        }
        Ok(())
    }
}

/// A `BufRead` of the texts the other end's [`Sealer`] wrote on `R`, frames opened as they come.
pub(super) struct Opened<R> {
    stream: R,
    keys: Arc<Keys>,
    nonce: u64,
    /// The frame last read, and its text up to `size`, of which `at` on is unread.
    frame: Vec<u8>,
    text: Vec<u8>,
    at: usize,
    size: usize,
}

impl<R: Read> Opened<R> {
    pub(super) fn new(stream: R, keys: Arc<Keys>) -> Self {
        Self {
            stream,
            keys,
            nonce: 0,
            frame: vec![0; MOST],
            text: vec![0; MOST],
            at: 0,
            size: 0,
        }
    }
}

impl<R: Read> Read for Opened<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let text = self.fill_buf()?;
        let size = out.len().min(text.len());
        out[..size].copy_from_slice(&text[..size]);
        self.consume(size);
        Ok(size)
    }
}

/// Fails with `InvalidData` at a frame that doesn't open, forged or changed on the way.
impl<R: Read> BufRead for Opened<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.at == self.size {
            let Some(size) = read_frame(&mut self.stream, &mut self.frame)? else {
                return Ok(&[]);
            };
            self.size = (self.keys)
                .read_message(self.nonce, &self.frame[..size], &mut self.text)
                .map_err(|_| {
                    let why = "a frame that doesn't open, forged or changed on the way";
                    io::Error::new(ErrorKind::InvalidData, why)
                })?;
            self.nonce += 1;
            self.at = 0;
        }
        Ok(&self.text[self.at..self.size])
    }

    fn consume(&mut self, amount: usize) {
        self.at = (self.at + amount).min(self.size);
    }
}

/// Seals `text`, at most `MOST - TAG` bytes, under `keys` with `nonce` into `packet`: the nonce,
/// then the sealed text.
pub(super) fn seal_packet(keys: &Keys, nonce: u64, text: &[u8], packet: &mut Vec<u8>) {
    packet.clear();
    packet.extend_from_slice(&nonce.to_be_bytes());
    packet.resize(SEALED + text.len(), 0);
    let sealed = keys.write_message(nonce, text, &mut packet[8..]);
    sealed.expect("a packet's text is fewer bytes than a message's");
}

/// Opens `packet` ([`seal_packet`]): its nonce and text. `None` if it doesn't open, as one forged,
/// changed on the way or sealed under other keys doesn't.
pub(super) fn open_packet(keys: &Keys, packet: &[u8]) -> Option<(u64, Vec<u8>)> {
    let (nonce, sealed) = packet.split_first_chunk::<8>()?;
    let nonce = u64::from_be_bytes(*nonce);
    let mut text = vec![0; sealed.len().checked_sub(TAG)?];
    let size = keys.read_message(nonce, sealed, &mut text).ok()?;
    text.truncate(size);
    Some((nonce, text))
}

/// The nonces of the packets a link has taken, in a window as IPsec and WireGuard keep them (RFC
/// 6479): the newest, and which of the 127 before it were taken. A replayed packet is dropped; one
/// out of order within the window is taken; one further behind is dropped too, as a later packet
/// carries what it did.
#[derive(Default)]
pub(super) struct Replays {
    newest: Option<u64>,
    /// A bit for the newest (lowest) and each of the 127 before it, set for those taken.
    taken: u128,
}

impl Replays {
    /// Whether `nonce`, of a packet that opened, is to be taken, which it then is: [`PACKETS`] or
    /// past, not taken before, and less than 128 behind the newest.
    pub(super) fn fresh(&mut self, nonce: u64) -> bool {
        if nonce < PACKETS {
            return false;
        }
        let Some(newest) = self.newest.filter(|&newest| nonce <= newest) else {
            let ahead = self.newest.map_or(u64::MAX, |newest| nonce - newest);
            let kept = if ahead < 128 { self.taken << ahead } else { 0 };
            self.taken = kept | 1;
            self.newest = Some(nonce);
            return true;
        };
        let behind = newest - nonce;
        let bit = 1u128.checked_shl(behind.try_into().unwrap_or(u32::MAX));
        match bit {
            Some(bit) if self.taken & bit == 0 => {
                self.taken |= bit;
                true
            }
            _ => false,
        }
    }
}

/// The ends of a loopback connection (maker, then taker) with their keys, keyed by [`TESTED`].
#[cfg(test)]
pub(super) fn linked() -> [(std::net::TcpStream, Keys); 2] {
    linked_by(&TESTED, &TESTED).map(|end| end.expect("a handshake"))
}

/// As [`linked`], the maker given `making` and the taker `taking`; each end may fail instead.
#[cfg(test)]
pub(super) fn linked_by(
    making: &Code,
    taking: &Code,
) -> [io::Result<(std::net::TcpStream, Keys)>; 2] {
    use std::net::{TcpListener, TcpStream};

    let listener = TcpListener::bind("127.0.0.1:0").expect("listening");
    let at = listener.local_addr().expect("an address");
    std::thread::scope(|scope| {
        let made = scope.spawn(|| {
            let mut stream = TcpStream::connect(at)?;
            handshake(&mut stream, making, false).map(|keys| (stream, keys))
        });
        let (mut stream, _) = listener.accept().expect("accepted");
        // Its end drops at a refusal before the other's is awaited, which then reads the close.
        let took = handshake(&mut stream, taking, true).map(|keys| (stream, keys));
        [made.join().expect("made"), took]
    })
}

#[cfg(test)]
pub(crate) const TESTED: Code = Code(*b"tested run");

#[cfg(test)]
mod tests {
    use super::*;

    /// A code is written in fours and read back whatever its case, with `I`, `L` and `O` for `1`,
    /// `1` and `0` and spaces for hyphens; 15 or 17 symbols, or a `U`, isn't one.
    #[test]
    fn a_code_reads_back_as_it_s_written() {
        let code = Code([0x39, 0xe8, 0x51, 0x37, 0x4b, 0x1f, 0xa6, 0x9b, 0x5d, 0x42]);
        let written = code.to_string();
        assert_eq!(written, "77M5-2DTB-3YK9-PQA2");
        assert_eq!(written.parse(), Ok(code));
        assert_eq!("77m5 2dtb 3yk9 pqa2".parse(), Ok(code));
        assert_eq!(
            Code::from_str("0000-1111-0000-1111"),
            "oOoo-IiLl-OOOO-1lI1".parse()
        );
        let all = "ZZZZ-ZZZZ-ZZZZ-ZZZZ".parse::<Code>().expect("a code");
        assert_eq!(all, Code([0xff; 10]));
        assert_eq!(all.to_string(), "ZZZZ-ZZZZ-ZZZZ-ZZZZ");
        for wrong in [
            "77M5-2DTB-3YK9-PQA",
            "77M5-2DTB-3YK9-PQA22",
            "77M5-2DTB-3YK9-PQAU",
            "",
        ] {
            assert!(wrong.parse::<Code>().is_err(), "{wrong}");
        }
        assert_eq!(format!("{code:?}"), "Code(..)");
        // Made from the OS's randomness, each its own.
        assert_ne!(Code::make().expect("a code"), Code::make().expect("a code"));
    }

    /// A code's room at a relay is its own, the same each time, and another code's another. It's
    /// none of the code's key, which the relay, seeing the room, mustn't learn.
    #[test]
    fn a_code_s_room_is_its_own_and_not_its_key() {
        let code = Code([0x39, 0xe8, 0x51, 0x37, 0x4b, 0x1f, 0xa6, 0x9b, 0x5d, 0x42]);
        let other = Code([0; 10]);
        assert_eq!(code.room(), code.room());
        assert_ne!(code.room(), other.room());
        let key = code.key();
        assert!(key.windows(16).all(|part| part != code.room().0), "{key:?}");
    }

    /// Each nonce is taken once, from [`PACKETS`] on: one 127 behind the newest is taken, not one
    /// 128 behind, and after a jump ahead the window moves with it.
    #[test]
    fn replays_take_each_packet_s_nonce_once() {
        let mut replays = Replays::default();
        assert!(!replays.fresh(PACKETS - 1));
        assert!(replays.fresh(PACKETS + 200));
        assert!(!replays.fresh(PACKETS + 200));
        assert!(replays.fresh(PACKETS + 200 - 127));
        assert!(!replays.fresh(PACKETS + 200 - 127));
        assert!(!replays.fresh(PACKETS + 200 - 128));
        assert!(replays.fresh(PACKETS + 201));
        assert!(replays.fresh(PACKETS + 199));
        // The window moves with a jump, those within it kept.
        assert!(replays.fresh(PACKETS + 300));
        assert!(!replays.fresh(PACKETS + 201));
        assert!(replays.fresh(PACKETS + 250));
        assert!(!replays.fresh(PACKETS + 250));
        assert!(replays.fresh(u64::MAX - 1));
        assert!(!replays.fresh(PACKETS + 300));
    }

    /// Ends given the same code make keys that open what the other seals, frames and packets. Given
    /// another, the end that took the connection refuses the first message and the end that made it
    /// finds it closed.
    #[test]
    fn a_handshake_keys_its_link_by_the_code() {
        let [(_, making), (_, taking)] = linked();
        let (making, taking) = (Arc::new(making), Arc::new(taking));
        let mut stream = Vec::new();
        let mut sealer = Sealer::new(Arc::clone(&making));
        let long = "x".repeat(MOST * 2);
        for text in ["hello\n", long.as_str(), "end\n"] {
            sealer.write(&mut stream, text.as_bytes()).expect("written");
        }
        let mut opened = String::new();
        Opened::new(&stream[..], Arc::clone(&taking))
            .read_to_string(&mut opened)
            .expect("opened");
        assert_eq!(opened, format!("hello\n{long}end\n"));
        // Nothing of the text shows.
        assert!(!stream.windows(5).any(|bytes| bytes == b"hello"));
        // A frame changed on the way ends the stream.
        let mut changed = stream.clone();
        changed[5] ^= 1;
        let error = Opened::new(&changed[..], Arc::clone(&taking))
            .read_to_string(&mut String::new())
            .expect_err("refused");
        assert_eq!(error.kind(), ErrorKind::InvalidData);

        let mut packet = Vec::new();
        seal_packet(&making, PACKETS, b"a 0 0 0 - 0 - -\n", &mut packet);
        assert_eq!(packet.len(), SEALED + 16);
        let opened = open_packet(&taking, &packet);
        assert_eq!(opened, Some((PACKETS, b"a 0 0 0 - 0 - -\n".to_vec())));
        // Sent back to its sender, changed, or under another nonce, it doesn't open.
        assert_eq!(open_packet(&making, &packet), None);
        let mut changed = packet.clone();
        changed[12] ^= 1;
        assert_eq!(open_packet(&taking, &changed), None);
        changed = packet.clone();
        changed[7] ^= 1;
        assert_eq!(open_packet(&taking, &changed), None);
        assert_eq!(open_packet(&taking, &packet[..SEALED - 1]), None);

        let other = Code(*b"other run!");
        let [Err(made), Err(took)] = linked_by(&other, &TESTED) else {
            panic!("a run of another code was taken");
        };
        assert_eq!(took.kind(), ErrorKind::InvalidData, "{took}");
        assert_eq!(made.kind(), ErrorKind::UnexpectedEof, "{made}");
    }
}
