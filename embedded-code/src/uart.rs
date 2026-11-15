//! Interrupt-driven UART0 driver for the Stellaris LM3S6965, with fixed-size
//! ring buffers for sending and receiving.

use crate::mutex::Mutex;
use core::fmt::Write;
use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
use tudelft_lm3s6965_pac::interrupt;
use tudelft_lm3s6965_pac::{uart0, UART0};

/// Sizes of the TX and RX ring buffers. A frame is at most 67 bytes
/// (64 payload + 3), so each buffer holds a few frames.
const TX_BUFFER_SIZE: usize = 256;
const RX_BUFFER_SIZE: usize = 256;

/// The baud rate divisors assume a 50 MHz system clock, the chip's maximum.
/// Nothing here sets up the PLL that the real chip needs for that, but QEMU
/// ignores the divisors and sends each byte as soon as it is written.
const SYSTEM_CLOCK: u32 = 50_000_000;

const BAUD_RATE: u32 = 115_200;

/// Bytes waiting to be sent, shared with the interrupt handler.
static TX_BUFFER: Mutex<ConstGenericRingBuffer<u8, TX_BUFFER_SIZE>> =
    Mutex::new(ConstGenericRingBuffer::<u8, TX_BUFFER_SIZE>::new());

/// Received bytes, shared with the interrupt handler.
static RX_BUFFER: Mutex<ConstGenericRingBuffer<u8, RX_BUFFER_SIZE>> =
    Mutex::new(ConstGenericRingBuffer::<u8, RX_BUFFER_SIZE>::new());

/// UART0 driver. The interrupt handler moves received bytes into RX_BUFFER for
/// `read`. `write` queues bytes in TX_BUFFER and fills the hardware FIFO, and
/// the interrupt handler sends the rest as the FIFO drains.
pub struct Uart {
    /// The UART0 peripheral instance
    pub uart: UART0,
}

impl Uart {
    /// Set up UART0 for 115200 baud, 8 data bits, no parity, 1 stop bit
    /// (8-N-1), with FIFOs and the receive interrupt enabled, following
    /// section 12.4 of the LM3S6965 datasheet.
    pub fn new(uart: UART0) -> Self {
        // The UART is disabled while it is configured
        uart.ctl.write(|w| w.uart_ctl_uarten().clear_bit());

        // Baud rate divisor BRD = SYSTEM_CLOCK / (16 * BAUD_RATE) = 27.13.
        // IBRD gets the integer part (27) and FBRD the fraction in 64ths,
        // rounded: round(0.13 * 64) = 8.
        let brd_denominator = 16 * BAUD_RATE;
        let brd_int = (SYSTEM_CLOCK / brd_denominator) as u16;
        let remainder = SYSTEM_CLOCK % brd_denominator;
        let brd_frac = ((remainder * 64 + brd_denominator / 2) / brd_denominator) as u8;

        // SAFETY: bits() is unsafe because the PAC can't tell which values are
        // valid for these fields. DIVINT is 16 bits and DIVFRAC 6 bits, so 27
        // and 8 fit, and the UART is disabled above.
        uart.ibrd
            .write(|w| unsafe { w.uart_ibrd_divint().bits(brd_int) });
        uart.fbrd
            .write(|w| unsafe { w.uart_fbrd_divfrac().bits(brd_frac) });

        // 8 data bits and FIFOs on. The register write starts from the reset
        // value (0), so parity, the second stop bit and break stay off. The new
        // divisors only take effect after this write to LCRH.
        uart.lcrh.write(|w| {
            w.uart_lcrh_wlen()
                .uart_lcrh_wlen_8()
                .uart_lcrh_fen()
                .set_bit()
        });

        // Interrupt when data arrives. The TX interrupt is turned on by
        // Uart::write when there is more to send than fits in the FIFO.
        uart.im.modify(|_, w| w.uart_im_rxim().set_bit());

        // Enable UART, transmitter, and receiver
        uart.ctl.modify(|_, w| {
            w.uart_ctl_uarten()
                .set_bit()
                .uart_ctl_txe()
                .set_bit()
                .uart_ctl_rxe()
                .set_bit()
        });

        // SAFETY: unmask is unsafe because it can break critical sections that
        // work by masking this interrupt in the NVIC. There are none: Mutex
        // disables all interrupts instead, which unmasking doesn't affect.
        unsafe {
            cortex_m::peripheral::NVIC::unmask(tudelft_lm3s6965_pac::Interrupt::UART0);
        }

        Uart { uart }
    }

    /// Queue bytes to be sent. If they don't all fit in the TX buffer nothing
    /// is queued, so a frame is never sent half.
    pub fn write(&mut self, value: &[u8]) {
        TX_BUFFER.update(|buf| {
            // enqueue overwrites the oldest byte when the buffer is full,
            // so only add the bytes if they all fit
            if buf.capacity() - buf.len() < value.len() {
                return;
            }
            for &byte in value {
                buf.enqueue(byte);
            }
            // Start sending. The TX interrupt only comes when the FIFO drains,
            // so it is only needed when not everything fit in the FIFO.
            if fill_tx_fifo(&self.uart, buf) {
                self.uart.im.modify(|_, w| w.uart_im_txim().set_bit());
            }
        });
    }

    /// Take the next received byte, if there is one.
    pub fn read(&mut self) -> Option<u8> {
        RX_BUFFER.update(|buf| buf.dequeue())
    }

    /// Whether there are received bytes that haven't been read yet.
    pub fn has_data(&self) -> bool {
        RX_BUFFER.update(|buf| !buf.is_empty())
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write(s.as_bytes());
        Ok(())
    }
}

/// Move bytes from the TX buffer into the hardware FIFO until the FIFO is full
/// or the buffer is empty, and return whether bytes are left. Only called
/// inside `TX_BUFFER.update`, so `write` and the interrupt handler never send
/// at the same time and the bytes go out in order.
fn fill_tx_fifo(
    uart: &uart0::RegisterBlock,
    buf: &mut ConstGenericRingBuffer<u8, TX_BUFFER_SIZE>,
) -> bool {
    while uart.fr.read().uart_fr_txff().bit_is_clear() {
        let Some(byte) = buf.dequeue() else {
            return false;
        };
        // SAFETY: bits() is unsafe because the PAC can't tell which values are
        // valid. DATA is 8 bits wide, so every byte is.
        uart.dr.write(|w| unsafe { w.uart_dr_data().bits(byte) });
    }
    !buf.is_empty()
}

/// UART0 interrupt handler: moves received bytes from the RX FIFO into
/// RX_BUFFER, and refills the TX FIFO from TX_BUFFER.
#[interrupt]
unsafe fn UART0() {
    // SAFETY: UART0::ptr() points to the UART0 registers, which exist for the
    // whole program. The Uart in the main code owns UART0, but after new() it
    // only touches the registers inside TX_BUFFER.update, when this handler
    // can't run.
    let uart = &*UART0::ptr();

    let mis = uart.mis.read();

    if mis.uart_mis_rxmis().bit_is_set() {
        // Clear the interrupt before emptying the FIFO, not after. Otherwise a
        // byte that arrives just after the FIFO was found empty loses its
        // interrupt, and QEMU only raises it again when the FIFO goes from
        // empty to non-empty, so nothing would be received anymore.
        uart.icr.write(|w| w.uart_icr_rxic().set_bit());
        while uart.fr.read().uart_fr_rxfe().bit_is_clear() {
            let byte = uart.dr.read().uart_dr_data().bits();
            // When the buffer is full the byte is dropped (enqueue would
            // overwrite the oldest one instead), and the parser rejects the
            // frame it belonged to
            RX_BUFFER.update(|buf| {
                if !buf.is_full() {
                    buf.enqueue(byte);
                }
            });
        }
    }

    if mis.uart_mis_txmis().bit_is_set() {
        uart.icr.write(|w| w.uart_icr_txic().set_bit());
        // Turn the TX interrupt off once everything has been sent
        if !TX_BUFFER.update(|buf| fill_tx_fifo(uart, buf)) {
            uart.im.modify(|_, w| w.uart_im_txim().clear_bit());
        }
    }

    // Only RXIM and TXIM are enabled, so there are no other interrupt flags
    // to clear here
}
