// Tests for the protocol (framing, checksum, parser).
// The workspace builds for ARM by default so these have to run on the host:
// cargo test -p comm_protocol --target x86_64-unknown-linux-gnu

use comm_protocol::{
    crc8, decode_command, decode_response, encode_frame, Command, FrameParser, Response, ViewPage,
    MAX_PAYLOAD_SIZE, START_BYTE,
};
use serde::Serialize;

/// Same size as response_buf in embedded-code/src/main.rs
const FIRMWARE_RESPONSE_BUF: usize = 64;

fn all_commands() -> Vec<Command> {
    vec![
        Command::Step { dx: 0, dy: 0 },
        Command::Step { dx: 1, dy: -1 },
        Command::Step {
            dx: i16::MIN,
            dy: i16::MAX,
        },
        Command::SwitchView {
            page: ViewPage::Map,
        },
        Command::SwitchView {
            page: ViewPage::Counter,
        },
        Command::GetSteps,
        Command::Reset,
    ]
}

fn all_responses() -> Vec<Response> {
    vec![
        Response::Ack,
        Response::StepCount { count: 0 },
        Response::StepCount { count: u32::MAX },
        Response::Error { code: 0 },
        Response::Error { code: u8::MAX },
    ]
}

/// Encode a message into a full frame
fn frame<T: Serialize>(msg: &T) -> Vec<u8> {
    let mut buf = [0u8; MAX_PAYLOAD_SIZE + 3];
    let len = encode_frame(msg, &mut buf).expect("message should fit in a frame");
    buf[..len].to_vec()
}

/// Feed bytes through a new parser and decode all accepted frames
fn parse_commands(bytes: &[u8]) -> Vec<Command> {
    let mut parser = FrameParser::new();
    bytes
        .iter()
        .filter_map(|&b| parser.feed(b).and_then(decode_command))
        .collect()
}

fn parse_responses(bytes: &[u8]) -> Vec<Response> {
    let mut parser = FrameParser::new();
    bytes
        .iter()
        .filter_map(|&b| parser.feed(b).and_then(decode_response))
        .collect()
}

// Checksum tests

#[test]
fn crc8_matches_the_standard_check_value() {
    // Our crc8 is CRC-8/SMBUS (poly 0x07, init 0, no reflection, no final xor),
    // its check value for "123456789" is 0xF4
    assert_eq!(crc8(b"123456789"), 0xF4);
    assert_eq!(crc8(&[]), 0x00);
}

#[test]
fn crc8_detects_every_single_bit_error() {
    let data = [0x00, 0x06, 0x03, START_BYTE, 0xFF, 0x42];
    let good = crc8(&data);
    for byte in 0..data.len() {
        for bit in 0..8 {
            let mut corrupted = data;
            corrupted[byte] ^= 1 << bit;
            assert_ne!(
                crc8(&corrupted),
                good,
                "flipping bit {bit} of byte {byte} was not detected"
            );
        }
    }
}

// Frame layout tests

#[test]
fn frame_is_start_byte_length_payload_checksum() {
    for cmd in all_commands() {
        let f = frame(&cmd);
        let len = f[1] as usize;
        assert_eq!(f[0], START_BYTE, "{cmd:?}");
        assert!((1..=MAX_PAYLOAD_SIZE).contains(&len), "{cmd:?}");
        assert_eq!(f.len(), len + 3, "{cmd:?}");
        assert_eq!(f[len + 2], crc8(&f[2..2 + len]), "{cmd:?}");
    }
}

#[test]
fn wire_format_is_stable() {
    // postcard writes the variant index as varint and i16 as zigzag varint
    // if this fails the runner and firmware don't use the same format anymore
    assert_eq!(
        frame(&Command::GetSteps),
        [START_BYTE, 0x01, 0x02, crc8(&[0x02])]
    );
    assert_eq!(
        frame(&Command::Step { dx: 3, dy: -2 }),
        [START_BYTE, 0x03, 0x00, 0x06, 0x03, 0x77]
    );
}

#[test]
fn every_response_fits_in_the_firmware_response_buffer() {
    let mut buf = [0u8; FIRMWARE_RESPONSE_BUF];
    for response in all_responses() {
        assert!(
            encode_frame(&response, &mut buf).is_some(),
            "{response:?} does not fit in {FIRMWARE_RESPONSE_BUF} bytes"
        );
    }
}

// Round trip tests

#[test]
fn every_command_survives_encode_parse_decode() {
    for cmd in all_commands() {
        assert_eq!(parse_commands(&frame(&cmd)), vec![cmd]);
    }
}

#[test]
fn every_response_survives_encode_parse_decode() {
    for response in all_responses() {
        assert_eq!(parse_responses(&frame(&response)), vec![response]);
    }
}

#[test]
fn back_to_back_frames_are_decoded_in_order() {
    let commands = all_commands();
    let stream: Vec<u8> = commands.iter().flat_map(frame).collect();
    assert_eq!(parse_commands(&stream), commands);
}

#[test]
fn parser_returns_a_payload_only_on_the_checksum_byte() {
    let f = frame(&Command::Reset);
    let mut parser = FrameParser::new();
    for &b in &f[..f.len() - 1] {
        assert!(parser.feed(b).is_none());
    }
    assert!(parser.feed(f[f.len() - 1]).is_some());
}

#[test]
fn default_values() {
    assert_eq!(ViewPage::default(), ViewPage::Map);
    let mut parser = FrameParser::default();
    let accepted: Vec<Command> = frame(&Command::GetSteps)
        .into_iter()
        .filter_map(|b| parser.feed(b).and_then(decode_command))
        .collect();
    assert_eq!(accepted, vec![Command::GetSteps]);
}

// Limit tests

#[test]
fn largest_payload_is_accepted() {
    let payload = [0x5A; MAX_PAYLOAD_SIZE];
    let mut f = vec![START_BYTE, MAX_PAYLOAD_SIZE as u8];
    f.extend_from_slice(&payload);
    f.push(crc8(&payload));

    let mut parser = FrameParser::new();
    let mut received = None;
    for &b in &f {
        if let Some(p) = parser.feed(b) {
            received = Some(p.to_vec());
        }
    }
    assert_eq!(received.as_deref(), Some(&payload[..]));
}

#[test]
fn zero_or_too_large_length_resets_the_parser() {
    for bad_len in [0, MAX_PAYLOAD_SIZE as u8 + 1, 0xFF] {
        let mut stream = vec![START_BYTE, bad_len];
        stream.extend(frame(&Command::Reset));
        assert_eq!(
            parse_commands(&stream),
            vec![Command::Reset],
            "length byte {bad_len}"
        );
    }
}

#[test]
fn encoder_rejects_a_buffer_that_is_too_small() {
    let msg = Command::Step { dx: 3, dy: -2 };
    let needed = frame(&msg).len();

    let mut exact = vec![0u8; needed];
    assert_eq!(encode_frame(&msg, &mut exact), Some(needed));

    let mut one_short = vec![0u8; needed - 1];
    assert_eq!(encode_frame(&msg, &mut one_short), None);

    let mut tiny = [0u8; 2];
    assert_eq!(encode_frame(&Command::Reset, &mut tiny), None);
}

#[test]
fn encoder_rejects_payloads_larger_than_max() {
    // a byte slice is serialized as 1 length byte + the data
    let mut buf = [0u8; 2 * MAX_PAYLOAD_SIZE];
    let fits = [0u8; MAX_PAYLOAD_SIZE - 1];
    let too_big = [0u8; MAX_PAYLOAD_SIZE];
    assert_eq!(
        encode_frame(&&fits[..], &mut buf),
        Some(MAX_PAYLOAD_SIZE + 3)
    );
    assert_eq!(encode_frame(&&too_big[..], &mut buf), None);
}

#[test]
fn invalid_payloads_do_not_decode() {
    assert_eq!(decode_command(&[]), None);
    assert_eq!(decode_command(&[0x04]), None); // no fifth command
    assert_eq!(decode_command(&[0x00]), None); // Step without dx/dy
    assert_eq!(decode_command(&[0x01, 0x02]), None); // no third view page
    assert_eq!(decode_response(&[0x03]), None); // no fourth response
}

// Fault tolerance tests

#[test]
fn noise_before_a_frame_is_skipped() {
    let mut stream = vec![0x00, 0x13, 0x37, 0xFF, 0x42];
    stream.extend(frame(&Command::GetSteps));
    assert_eq!(parse_commands(&stream), vec![Command::GetSteps]);
}

#[test]
fn corrupted_payload_is_rejected_and_the_next_frame_is_decoded() {
    let mut stream = frame(&Command::Step { dx: 3, dy: -2 });
    stream[3] ^= 0x01;
    stream.extend(frame(&Command::Reset));
    assert_eq!(parse_commands(&stream), vec![Command::Reset]);
}

#[test]
fn wrong_checksum_is_rejected_and_the_next_frame_is_decoded() {
    let mut stream = frame(&Command::GetSteps);
    let last = stream.len() - 1;
    stream[last] ^= 0xFF;
    stream.extend(frame(&Command::Reset));
    assert_eq!(parse_commands(&stream), vec![Command::Reset]);
}

#[test]
fn reset_discards_a_half_received_frame() {
    let mut parser = FrameParser::new();
    for &b in &frame(&Command::Step { dx: 1, dy: 1 })[..3] {
        assert!(parser.feed(b).is_none());
    }
    parser.reset();

    let accepted: Vec<Command> = frame(&Command::Reset)
        .into_iter()
        .filter_map(|b| parser.feed(b).and_then(decode_command))
        .collect();
    assert_eq!(accepted, vec![Command::Reset]);
}

#[test]
fn repeated_start_byte_does_not_lose_the_frame() {
    let mut stream = vec![START_BYTE];
    stream.extend(frame(&Command::GetSteps));
    assert_eq!(parse_commands(&stream), vec![Command::GetSteps]);
}

#[test]
fn dropped_byte_does_not_lose_the_next_frame() {
    // if a payload or checksum byte is lost, the start byte of the next frame
    // ends up where the checksum should be and the parser has to resync on it
    for cmd in all_commands() {
        let good = frame(&cmd);
        for i in 2..good.len() {
            let mut stream = good.clone();
            stream.remove(i);
            stream.extend(frame(&Command::Reset));
            assert_eq!(
                parse_commands(&stream),
                vec![Command::Reset],
                "{cmd:?} with byte {i} dropped"
            );
        }
    }
}

/// Fault injection: for every command, set each byte of the frame to every other
/// value and also try dropping it. Checks that
/// 1. a broken frame never decodes into a different command
/// 2. the next frame still arrives (except when the length byte was hit)
/// 3. the parser gets back in sync so later frames are decoded again
#[test]
fn every_single_byte_fault_is_rejected_and_the_parser_recovers() {
    let right_after = Command::Step { dx: 7, dy: 7 };
    let filler = Command::Step { dx: -7, dy: 7 };
    let last = Command::Step { dx: 7, dy: -7 };

    // a broken length byte can make the parser wait for up to MAX_PAYLOAD_SIZE + 1
    // more bytes, so put enough frames after the fault
    let mut tail = frame(&right_after);
    while tail.len() < 2 * (MAX_PAYLOAD_SIZE + 3) {
        tail.extend(frame(&filler));
    }
    tail.extend(frame(&last));

    for victim in all_commands() {
        let good = frame(&victim);
        for i in 0..good.len() {
            let mut faults = Vec::new();
            for value in 0..=u8::MAX {
                if value != good[i] {
                    let mut f = good.clone();
                    f[i] = value;
                    faults.push(f);
                }
            }
            let mut dropped = good.clone();
            dropped.remove(i);
            faults.push(dropped);

            for fault in faults {
                let mut stream = fault.clone();
                stream.extend(&tail);
                let decoded = parse_commands(&stream);

                let allowed = [&victim, &right_after, &filler, &last];
                assert!(
                    decoded.iter().all(|c| allowed.contains(&c)),
                    "fault {fault:02X?} decoded as {decoded:?}"
                );
                if i != 1 {
                    assert!(
                        decoded.contains(&right_after),
                        "frame right after fault {fault:02X?} was lost"
                    );
                }
                assert_eq!(
                    decoded.last(),
                    Some(&last),
                    "no recovery after fault {fault:02X?}"
                );
            }
        }
    }
}
