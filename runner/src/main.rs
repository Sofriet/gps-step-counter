use std::env::args;
use std::io::{Read, Write};
use std::thread;

use std::thread::sleep;
use std::time::Duration;

use comm_protocol::{
    decode_response, encode_frame, Command, FrameParser, Response, ViewPage, MAX_PAYLOAD_SIZE,
};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use tudelft_arm_qemu_runner::Runner;

/// Send a command to the board as one frame.
fn send(stream: &mut impl Write, cmd: &Command) -> std::io::Result<()> {
    let mut frame = [0u8; MAX_PAYLOAD_SIZE + 3];
    // Every command fits in a frame (checked in comm_protocol's tests)
    let len = encode_frame(cmd, &mut frame).expect("command does not fit in a frame");
    stream.write_all(&frame[..len])
}

fn print_help() {
    println!();
    println!("=== GPS Step Counter - Command Reference ===");
    println!();
    println!("  step <dx> <dy>  - Record a step with position change (meters)");
    println!("                    Example: step 1 0  (move 1 meter right)");
    println!("                    Example: step 0 -1 (move 1 meter up)");
    println!();
    println!("  steps           - Get the current step count from device");
    println!();
    println!("  reset           - Reset device state (position and step count)");
    println!();
    println!("  view map        - Switch to map view (shows walking path)");
    println!("  view counter    - Switch to step counter view");
    println!();
    println!("  help            - Show this help message");
    println!("  quit            - Quit the program");
    println!();
    println!("============================================");
    println!();
}

fn main() -> color_eyre::Result<()> {
    tracing_subscriber::fmt::init();
    color_eyre::install()?;

    let binary = args().nth(1).expect("Usage: runner <path-to-binary>");
    let mut runner: Runner = Runner::new(&binary, false)?;

    // Set stream to non-blocking for reading
    runner.stream.set_nonblocking(true)?;

    let mut parser = FrameParser::new();
    let mut rl = DefaultEditor::new()?;

    println!();
    println!("GPS Step Counter Runner");
    println!("Type 'help' for available commands");
    println!();

    // Read initial output from device
    thread::sleep(Duration::from_millis(500));
    let mut buf = [0u8; 256];
    loop {
        match runner.stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                print!("{}", String::from_utf8_lossy(&buf[..n]));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e.into()),
        }
    }

    loop {
        sleep(Duration::from_millis(100));
        // Check for incoming data from device
        match runner.stream.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                for &byte in &buf[..n] {
                    if let Some(response) = parser.feed(byte).and_then(decode_response) {
                        match response {
                            Response::StepCount { count } => {
                                println!("[Device] Step count: {}\n", count);
                            }
                            Response::Ack => {
                                println!("[Device] Command acknowledged\n");
                            }
                            Response::Error { code } => {
                                println!("[Device] Error: code {}\n", code);
                            }
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }

        // Read command from user
        let readline = rl.readline(">> ");
        match readline {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(line);
                let parts: Vec<&str> = line.split_whitespace().collect();

                match parts.as_slice() {
                    ["help"] | ["h"] | ["?"] => {
                        print_help();
                    }
                    ["quit"] | ["exit"] | ["q"] => {
                        println!("Goodbye!");
                        break;
                    }
                    ["step", dx, dy] => {
                        if let (Ok(dx), Ok(dy)) = (dx.parse::<i16>(), dy.parse::<i16>()) {
                            send(&mut runner.stream, &Command::Step { dx, dy })?;
                            println!("Sent step command: dx={}, dy={}", dx, dy);
                        } else {
                            println!("Invalid arguments. Usage: step <dx> <dy>");
                        }
                    }
                    ["steps"] | ["count"] | ["getsteps"] => {
                        send(&mut runner.stream, &Command::GetSteps)?;
                        println!("Requested step count...");
                    }
                    ["reset"] => {
                        send(&mut runner.stream, &Command::Reset)?;
                        println!("Sent reset command");
                    }
                    ["view", "map"] => {
                        let page = ViewPage::Map;
                        send(&mut runner.stream, &Command::SwitchView { page })?;
                        println!("Switched to map view");
                    }
                    ["view", "counter"] | ["view", "steps"] => {
                        let page = ViewPage::Counter;
                        send(&mut runner.stream, &Command::SwitchView { page })?;
                        println!("Switched to counter view");
                    }
                    _ => {
                        println!(
                            "Unknown command: '{}'. Type 'help' for available commands.",
                            line
                        );
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("CTRL-C");
                break;
            }
            Err(ReadlineError::Eof) => {
                println!("CTRL-D");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }

    Ok(())
}
