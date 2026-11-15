use crate::drawing::brightness::Brightness;
use crate::drawing::font::Character;
use tudelft_lm3s6965_pac::{GPIO_PORTC, SSI0};

pub struct Screen<'p> {
    ssi: &'p mut SSI0,
    gpio: &'p mut GPIO_PORTC,
    fb: [[u8; (Screen::WIDTH / 2) as usize]; Screen::HEIGHT as usize],
}

impl<'p> Screen<'p> {
    pub const WIDTH: u8 = 128;
    pub const HEIGHT: u8 = 80;

    pub fn new(ssi: &'p mut SSI0, gpio: &'p mut GPIO_PORTC) -> Self {
        // 1. Ensure that the SSE bit in the SSICR1 register is disabled before making any configuration changes.
        ssi.cr1.write(|w| w.ssi_cr1_sse().clear_bit());

        // 2. Select whether the SSI is a master or slave:
        //     a. For master operations, set the SSICR1 register to 0x0000.0000.
        //     b. For slave mode (output enabled), set the SSICR1 register to 0x0000.0004.
        //     c. For slave mode (output disabled), set the SSICR1 register to 0x0000.000C.
        ssi.cr1.write(|w| w.ssi_cr1_ms().clear_bit());

        // 3. Configure the clock prescale divisor by writing the SSICPSR register
        //
        // SAFETY: bits() is unsafe because the PAC can't tell which values are
        // valid. CPSDVSR must be an even number from 2 to 254 (SSICPSR,
        // chapter 13), so 2 is valid, and SSE is 0 as 13.4 requires.
        ssi.cpsr.write(|w| unsafe { w.ssi_cpsr_cpsdvsr().bits(2) });

        // 4. Write the SSICR0 register with the following configuration:
        //     ■ Serial clock rate (SCR)
        //     ■ Desired clock phase/polarity, if using Freescale SPI mode (SPH and SPO)
        //     ■ The protocol mode: Freescale SPI, TI SSF, MICROWIRE (FRF)
        //     ■ The data size (DSS)
        //
        // SCR and DSS are set in one write: write() starts from the reset
        // value, so a second write would clear SCR again. The other fields keep
        // their reset values (Freescale SPI, SPO = 0, SPH = 0), and
        // SSIClk = SysClk / (CPSDVSR * (1 + SCR)).
        //
        // SAFETY: bits() is unsafe because the PAC can't tell which values are
        // valid. SCR is 8 bits (0 to 255), so 9 is valid, and SSE is still 0.
        ssi.cr0.write(|w| {
            unsafe { w.ssi_cr0_scr().bits(9) };
            w.ssi_cr0_dss().ssi_cr0_dss_16()
        });

        // Enable the SSI by setting the SSE bit in the SSICR1 register.
        ssi.cr1.write(|w| w.ssi_cr1_sse().set_bit());

        // DIR = 0xFF makes all port C pins outputs. PC7 is the display's
        // data/command line (see change_mode). DEN = 1 only turns on the
        // digital function of PC0: QEMU ignores DEN, but on the real chip PC7
        // would need its DEN bit set as well.
        //
        // SAFETY: bits() writes the whole register. Both registers have one
        // bit per pin, and these values leave the reserved bits 0.
        gpio.den.write(|w| unsafe { w.bits(1) });
        gpio.dir.write(|w| unsafe { w.bits(0xff) });

        Self {
            ssi,
            gpio,
            fb: [[0; (Self::WIDTH / 2) as usize]; Self::HEIGHT as usize],
        }
    }

    fn write_ssi(&mut self, data: u16) {
        // SAFETY: bits() is unsafe because the PAC can't tell which values are
        // valid. DATA is 16 bits wide and new() sets 16-bit frames (DSS), so
        // every u16 is a valid frame.
        self.ssi.dr.write(|w| unsafe { w.ssi_dr_data().bits(data) });
        let _ = self.ssi.dr.read();
        while self.ssi.sr.read().ssi_sr_bsy().bit_is_set() {}
    }

    fn change_mode(&mut self, mode: Mode) {
        // The board uses 0x00 for command mode and 0xA0 for data mode (PC7 is
        // the display's data/command line).
        //
        // SAFETY: bits() writes the whole register. GPIODATA has one bit per
        // pin, and 0x00 and 0xA0 leave the reserved bits 0.
        match mode {
            Mode::Cmd => self.gpio.data.write(|w| unsafe { w.bits(0x00) }),
            Mode::Data => self.gpio.data.write(|w| unsafe { w.bits(0xa0) }),
        }
    }

    /// Assumes we are in command mode and min/max are in bounds
    fn set_col(&mut self, min_curr: u8, max: u8) {
        self.write_ssi(0x15);
        self.write_ssi(min_curr as u16);
        self.write_ssi(max as u16);
    }

    /// Assumes we are in command mode and min/max are in bounds
    fn set_row(&mut self, min_curr: u8, max: u8) {
        self.write_ssi(0x75);
        self.write_ssi(min_curr as u16);
        self.write_ssi(max as u16);
    }

    pub fn draw_pixel(&mut self, x: u8, y: u8, brightness: Brightness) {
        assert!(x < Self::WIDTH, "x larger than width");
        assert!(y < Self::HEIGHT, "y larger than height");

        self.change_mode(Mode::Cmd);
        self.set_col(x / 2, Self::WIDTH - 1);
        self.set_row(y, Self::HEIGHT - 1);

        self.change_mode(Mode::Data);

        let current = &mut self.fb[y as usize][x as usize / 2];
        if x % 2 == 1 {
            *current &= 0xf0;
            *current |= Into::<u8>::into(brightness);
        } else {
            *current &= 0x0f;
            *current |= Into::<u8>::into(brightness) << 4;
        }

        let value = *current;
        self.write_ssi(value as u16);
    }

    pub fn clear(&mut self, brightness: Brightness) {
        self.change_mode(Mode::Cmd);
        self.set_row(0, Self::HEIGHT - 1);
        self.set_col(0, Self::WIDTH - 1);

        self.change_mode(Mode::Data);

        let brightness: u8 = brightness.into();
        let pix = brightness | (brightness << 4);

        for x in 0..(Self::WIDTH / 2) as usize {
            for y in 0..Self::HEIGHT as usize {
                self.write_ssi(pix as u16);
                self.fb[y][x] = pix;
            }
        }
    }

    /// Draw a character from the font at the given position
    /// Character is 8 pixels wide and 16 pixels tall
    pub fn draw_character(&mut self, character: &Character, x: u8, y: u8, brightness: Brightness) {
        for (row, line) in character.iter().enumerate() {
            for (col, &pixel) in line.iter().enumerate() {
                if pixel {
                    let px = x + col as u8;
                    let py = y + row as u8;
                    if px < Self::WIDTH && py < Self::HEIGHT {
                        self.draw_pixel(px, py, brightness);
                    }
                }
            }
        }
    }
}

enum Mode {
    Cmd,
    Data,
}
