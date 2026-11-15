#![no_std]
#![no_main]

extern crate cortex_m_rt as rt;
extern crate tudelft_lm3s6965_pac;

use crate::drawing::font::DIGITS;
use crate::status::Status;
use crate::uart::Uart;
use comm_protocol::{decode_command, encode_frame, Command, FrameParser, Response, ViewPage};
use core::fmt::Write;
use cortex_m::{asm, interrupt};
use cortex_m_semihosting::hprintln;
use drawing::brightness::Brightness;
use drawing::screen::Screen;
use rt::entry;
use tudelft_lm3s6965_pac::Peripherals;

mod drawing;
mod exceptions;
mod mutex;
mod status;
mod uart;

#[entry]
fn main() -> ! {
    hprintln!("GPS Step Counter started, type 'help' for commands");
    let mut dp = Peripherals::take().unwrap();

    let mut screen = Screen::new(&mut dp.SSI0, &mut dp.GPIO_PORTC);
    screen.clear(Brightness::WHITE);
    let mut uart = Uart::new(dp.UART0);
    let mut state = Status::default();
    let mut parser = FrameParser::new();
    let mut response_buf = [0u8; 64];
    let mut needs_redraw = true;

    writeln!(&mut uart, "GPS Step Counter Ready").unwrap();

    loop {
        while let Some(byte) = uart.read() {
            if let Some(payload) = parser.feed(byte) {
                let response = match decode_command(payload) {
                    Some(Command::Step { dx, dy }) => {
                        state.record_step(dx, dy);
                        needs_redraw = true;
                        Response::Ack
                    }
                    Some(Command::SwitchView { page }) => {
                        state.switch_view(page);
                        needs_redraw = true;
                        Response::Ack
                    }
                    Some(Command::GetSteps) => Response::StepCount { count: state.steps },
                    Some(Command::Reset) => {
                        state.reset();
                        needs_redraw = true;
                        Response::Ack
                    }
                    None => {
                        hprintln!("Failed to decode command");
                        Response::Error { code: 1 }
                    }
                };
                if let Some(len) = encode_frame(&response, &mut response_buf) {
                    uart.write(&response_buf[..len]);
                }
            }
        }

        if needs_redraw {
            render_screen(&mut screen, &state);
            needs_redraw = false;
        }

        // Sleep until the next interrupt, but only if the RX buffer is empty:
        // bytes that arrived while the screen was drawn won't cause another
        // interrupt. The check runs with interrupts disabled, so a byte that
        // arrives between the check and wfi leaves its interrupt pending, and
        // a pending interrupt still wakes the core from wfi.
        interrupt::free(|_| {
            if !uart.has_data() {
                asm::wfi();
            }
        });
    }
}

/// Render the current view to the screen
fn render_screen(screen: &mut Screen, state: &Status) {
    screen.clear(Brightness::WHITE);
    match state.current_view {
        ViewPage::Map => render_map_view(screen, state),
        ViewPage::Counter => render_counter_view(screen, state),
    }
}

/// Render the map view showing the walking path
fn render_map_view(screen: &mut Screen, state: &Status) {
    // No bounds check needed, Status::record_step clamps positions to the screen
    for &(x, y) in state.path.iter() {
        screen.draw_pixel(x as u8, y as u8, Brightness::BLACK);
    }

    // Draw the current position too. It is normally the last point of the
    // path already, but once the path history is full it is no longer added.
    screen.draw_pixel(state.x as u8, state.y as u8, Brightness::BLACK);
}

/// Render the counter view: the step count, starting in the middle of the screen
fn render_counter_view(screen: &mut Screen, state: &Status) {
    draw_number(screen, state.steps, Screen::WIDTH / 2, Screen::HEIGHT / 2);
}

/// Draw a number with its first digit at (x, y). Digits are 8x16 pixels with
/// 1 pixel of space between them.
fn draw_number(screen: &mut Screen, num: u32, x: u8, y: u8) {
    let digits = num.checked_ilog10().unwrap_or(0) + 1;
    let mut rest = num;
    for i in (0..digits).rev() {
        draw_digit(screen, (rest % 10) as u8, x + 9 * i as u8, y);
        rest /= 10;
    }
}

/// Draw a single digit (0-9) at the given position using the font
fn draw_digit(screen: &mut Screen, digit: u8, x: u8, y: u8) {
    let character = &DIGITS[digit as usize];
    screen.draw_character(character, x, y, Brightness::BLACK);
}
