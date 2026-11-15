#![no_std]

use serde::{Deserialize, Serialize};

/// Start byte to mark beginning of a frame
pub const START_BYTE: u8 = 0xAA;

/// Maximum payload size
pub const MAX_PAYLOAD_SIZE: usize = 64;

/// Commands sent from runner to embedded device
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Record a step with position change (dx, dy in meters)
    Step { dx: i16, dy: i16 },
    /// Switch to a different view page
    SwitchView { page: ViewPage },
    /// Request the current step count (answered with StepCount instead of Ack)
    GetSteps,
    /// Reset all state to initial values
    Reset,
}

/// Responses sent from embedded device to runner
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    /// Acknowledgement of a command
    Ack,
    /// Current step count
    StepCount { count: u32 },
    /// A frame had a valid checksum but no valid command in it (code 1)
    Error { code: u8 },
}

/// Available view pages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ViewPage {
    /// Map view showing walking path
    #[default]
    Map = 0,
    /// Step counter display
    Counter = 1,
}

/// Calculate CRC-8 checksum (polynomial 0x07)
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x07;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// Frame parser state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseState {
    /// Waiting for start byte
    WaitingForStart,
    /// Received start byte, waiting for length
    WaitingForLength,
    /// Receiving payload bytes
    ReceivingPayload { remaining: usize },
    /// Waiting for checksum byte
    WaitingForChecksum,
}

/// Frame parser for incoming messages
pub struct FrameParser {
    state: ParseState,
    buffer: [u8; MAX_PAYLOAD_SIZE],
    buffer_len: usize,
}

impl FrameParser {
    pub const fn new() -> Self {
        FrameParser {
            state: ParseState::WaitingForStart,
            buffer: [0u8; MAX_PAYLOAD_SIZE],
            buffer_len: 0,
        }
    }

    /// Feed a byte to the parser. Returns Some(payload) when a complete valid frame is received.
    pub fn feed(&mut self, byte: u8) -> Option<&[u8]> {
        match self.state {
            ParseState::WaitingForStart => {
                if byte == START_BYTE {
                    self.state = ParseState::WaitingForLength;
                    self.buffer_len = 0;
                }
                None
            }
            ParseState::WaitingForLength => {
                if byte == START_BYTE {
                    // Repeated start byte (0xAA is never a valid length),
                    // treat this one as the real start
                    None
                } else if byte == 0 || byte as usize > MAX_PAYLOAD_SIZE {
                    // Invalid length, reset
                    self.reset();
                    None
                } else {
                    self.state = ParseState::ReceivingPayload {
                        remaining: byte as usize,
                    };
                    None
                }
            }
            ParseState::ReceivingPayload { remaining } => {
                // length is at most MAX_PAYLOAD_SIZE (checked above), so this fits
                self.buffer[self.buffer_len] = byte;
                self.buffer_len += 1;
                if remaining == 1 {
                    self.state = ParseState::WaitingForChecksum;
                } else {
                    self.state = ParseState::ReceivingPayload {
                        remaining: remaining - 1,
                    };
                }
                None
            }
            ParseState::WaitingForChecksum => {
                let buf_len = self.buffer_len;
                self.reset();
                let expected_crc = crc8(&self.buffer[..buf_len]);
                if byte == expected_crc {
                    Some(&self.buffer[..buf_len]) //frame accepted
                } else {
                    //frame discarded due to checksum mismatch
                    //if a byte got lost this can be the start byte of the next frame
                    if byte == START_BYTE {
                        self.state = ParseState::WaitingForLength;
                    }
                    None
                }
            }
        }
    }

    /// Reset the parser state (useful after errors)
    pub fn reset(&mut self) {
        self.state = ParseState::WaitingForStart;
        self.buffer_len = 0;
    }
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Encode a message into `buffer` as a frame: start byte, length, payload and
/// checksum. Used by both the firmware and the runner. Returns the frame
/// length, or None if the frame doesn't fit in `buffer` or the payload is
/// longer than MAX_PAYLOAD_SIZE.
pub fn encode_frame<T: Serialize>(msg: &T, buffer: &mut [u8]) -> Option<usize> {
    // Room for the start byte, the length and the checksum
    if buffer.len() < 3 {
        return None;
    }

    let buffer_len = buffer.len();
    let payload_len = postcard::to_slice(msg, &mut buffer[2..buffer_len - 1])
        .ok()?
        .len();
    if payload_len > MAX_PAYLOAD_SIZE {
        return None;
    }

    buffer[0] = START_BYTE;
    buffer[1] = payload_len as u8;
    buffer[2 + payload_len] = crc8(&buffer[2..2 + payload_len]);
    Some(3 + payload_len)
}

/// Decode a payload into a Command
pub fn decode_command(payload: &[u8]) -> Option<Command> {
    postcard::from_bytes(payload).ok()
}

/// Decode a payload into a Response
pub fn decode_response(payload: &[u8]) -> Option<Response> {
    postcard::from_bytes(payload).ok()
}
