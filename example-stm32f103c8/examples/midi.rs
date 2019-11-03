#![no_std]
#![no_main]

extern crate panic_semihosting;
// use cortex_m_semihosting::hprintln;

use cortex_m::asm::delay;
use cortex_m_rt::entry;

use stm32_usbd::UsbBus;

use stm32f1xx_hal::{prelude::*, stm32, timer::Timer};
use usb_device::prelude::*;

// const USB_CLASS_AUDIO: u8 = 0x01;

mod midi {
    use usb_device::class_prelude::*;
    use usb_device::Result;

    pub struct MidiClass<'a, B: UsbBus> {
        audio_if: InterfaceNumber,
        midi_if: InterfaceNumber,
        out_ep: EndpointOut<'a, B>,
        in_ep: EndpointIn<'a, B>,
    }

    impl<B: UsbBus> MidiClass<'_, B> {
        pub fn new(alloc: &UsbBusAllocator<B>) -> MidiClass<'_, B> {
            MidiClass {
                audio_if: alloc.interface(),
                midi_if: alloc.interface(),
                out_ep: alloc.bulk(64),
                in_ep: alloc.bulk(64),
            }
        }

        pub fn note_off(&self, chan: u8, key: u8, vel: u8) -> Result<usize> {
            // I have no idea why the "virtual cable" must be number 0 and not one of the jack IDs
            // but only 0 seemed to work
            self.in_ep.write(&[
                0x08, // note off
                0x80 | (chan & 0x0f),
                key & 0x7f,
                vel & 0x7f,
            ])
        }

        pub fn note_on(&self, chan: u8, key: u8, vel: u8) -> Result<usize> {
            self.in_ep.write(&[
                0x09,                 // note on
                0x90 | (chan & 0x0f), //
                key & 0x7f,
                vel & 0x7f,
            ])
        }

        pub fn control_msg(&self, chan: u8, ctrl_no: u8, ctrl_val: u8) -> Result<usize> {
            self.in_ep.write(&[
                0x0b,                 // ? used for naming ?
                0xb0 | (chan | 0x0f), // control message
                ctrl_no & 0x7f,       // controller number
                ctrl_val & 0x7f,      // controller value
            ])
        }
    }

    impl<B: UsbBus> UsbClass<B> for MidiClass<'_, B> {
        fn get_configuration_descriptors(&self, writer: &mut DescriptorWriter) -> Result<()> {
            writer.interface(self.audio_if, 0x01, 0x01, 0x00)?; // Interface 0
            writer.write(0x24, &[0x01, 0x00, 0x01, 0x09, 0x00, 0x01, 0x01])?; // CS Interface (audio)

            writer.interface(self.midi_if, 0x01, 0x03, 0x00)?; // Interface 1
            writer.write(0x24, &[0x01, 0x00, 0x01, 0x2e, 0x00])?; // CS Interface (midi)
            writer.write(0x24, &[0x02, 0x01, 0x01, 0x00])?; // IN Jack 1 (emb)
            writer.write(0x24, &[0x03, 0x01, 0x02, 0x01, 0x01, 0x01, 0x00])?; // OUT Jack 2 (emb)

            writer.endpoint(&self.out_ep)?;
            writer.write(0x25, &[0x01, 0x01, 0x01])?; // CS EP IN Jack

            writer.endpoint(&self.in_ep)?;
            writer.write(0x25, &[0x01, 0x01, 0x02])?; // CS EP OUT Jack

            Ok(())
        }

        fn endpoint_out(&mut self, ep: EndpointAddress) {
            if ep == self.out_ep.address() {
                let mut dummy = [0u8; 64];
                self.out_ep.read(&mut dummy).ok();
            }
        }
    }
}

// Fader Position
#[derive(Debug, PartialEq)]
enum Position {
    Left,
    Mid,
    Right,
}

impl Position {
    fn to_val(&self) -> u8 {
        match self {
            Position::Left => 0x00,
            Position::Mid => 0x40,
            Position::Right => 0x7F,
        }
    }

    fn new(a: bool, b: bool) -> Self {
        match (a, b) {
            (true, false) => Position::Left,
            (false, true) => Position::Right,
            (true, true) => Position::Mid,
            _ => panic!("invalid reading"),
        }
    }
}

use cortex_m::Peripherals;
#[cfg(feature = "itm-trace")]
use cortex_m::{iprint, iprintln};
#[cfg(not(feature = "itm-trace"))]
mod itm_stub {
    #[macro_export]
    macro_rules! iprint {
        ($channel:expr, $s:expr) => {};
        ($channel:expr, $($arg:tt)*) => {};
    }

    /// Macro for sending a formatted string through an ITM channel, with a newline.
    #[macro_export]
    macro_rules! iprintln {
        ($channel:expr) => {};
        ($channel:expr, $fmt:expr) => {};
        ($channel:expr, $fmt:expr, $($arg:tt)*) => {};
    }
}
use usbd_serial::{SerialPort, USB_CLASS_CDC};

#[entry]
fn main() -> ! {
    let dp = stm32::Peripherals::take().unwrap();

    let mut flash = dp.FLASH.constrain();
    let mut rcc = dp.RCC.constrain();

    let clocks = rcc
        .cfgr
        .use_hse(8.mhz())
        .sysclk(48.mhz())
        .pclk1(24.mhz())
        .freeze(&mut flash.acr);

    assert!(clocks.usbclk_valid());

    // Itm trace
    let mut cp = cortex_m::Peripherals::take().unwrap();
    // unsafe {
    //     // enable TPIU and ITM
    //     cp.DCB.demcr.modify(|r| r | (1 << 24));

    //     // prescaler
    //     let swo_freq = 2_000_000;
    //     cp.TPIU.acpr.write((clocks.sysclk().0 / swo_freq) - 1);

    //     // SWO NRZ
    //     cp.TPIU.sppr.write(2);

    //     cp.TPIU.ffcr.modify(|r| r & !(1 << 1));

    //     // STM32 specific: enable tracing in the DBGMCU_CR register
    //     const DBGMCU_CR: *mut u32 = 0xe0042004 as *mut u32;
    //     let r = core::ptr::read_volatile(DBGMCU_CR);
    //     core::ptr::write_volatile(DBGMCU_CR, r | (1 << 5));

    //     // unlock the ITM
    //     cp.ITM.lar.write(0xC5ACCE55);

    //     cp.ITM.tcr.write(
    //         (0b000001 << 16) | // TraceBusID
    //         (1 << 3) | // enable SWO output
    //         (1 << 0), // enable the ITM
    //     );

    //     // enable stimulus port 0
    //     cp.ITM.ter[0].write(1);
    // }
    let stim = &mut cp.ITM.stim[0];

    iprintln!(stim, "Reset, ITM tracing started");

    //hprintln!("here");
    //loop {}

    // Configure the on-board LED (PC13, green)
    let mut gpioc = dp.GPIOC.split(&mut rcc.apb2);
    let mut led = gpioc.pc13.into_push_pull_output(&mut gpioc.crh);
    led.set_high(); // Turn off

    // fader
    let in_a = gpioc.pc14.into_floating_input(&mut gpioc.crh);
    let in_b = gpioc.pc15.into_floating_input(&mut gpioc.crh);

    let mut gpioa = dp.GPIOA.split(&mut rcc.apb2);

    // BluePill board has a pull-up resistor on the D+ line.
    // Pull the D+ pin down to send a RESET condition to the USB bus.
    // 1/100 seconds = 10ms.qåcq
    let mut usb_dp = gpioa.pa12.into_push_pull_output(&mut gpioa.crh);
    usb_dp.set_low();
    iprintln!(stim, "sysclk {}", clocks.sysclk().0);
    //delay(clocks.sysclk().0 / 100);
    delay(clocks.sysclk().0 / 5);
    iprintln!(stim, "release low");

    let usb_dm = gpioa.pa11;
    let usb_dp = usb_dp.into_floating_input(&mut gpioa.crh);

    let usb_bus = UsbBus::new(dp.USB, (usb_dm, usb_dp));

    let mut midi = midi::MidiClass::new(&usb_bus);

    // https://github.com/obdev/v-usb/blob/master/usbdrv/USB-IDs-for-free.txt

    let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x05e4, 0x16c0))
        .manufacturer("Per.Lindgren@ltu.se")
        .product("Fader")
        .serial_number("v0.1")
        .build();

    iprintln!(stim, "start");

    let mut pos: Position = Position::new(in_a.is_high(), in_b.is_high());
    let mut init = true;
    loop {
        usb_dev.poll(&mut [&mut midi]);

        if usb_dev.state() == UsbDeviceState::Configured {
            let new_pos = Position::new(in_a.is_high(), in_b.is_high());

            if init || (new_pos != pos) {
                pos = new_pos;
                init = false;

                match pos {
                    Position::Mid => led.set_low(),
                    _ => led.set_high(),
                }

                iprintln!(stim, "pos: {:?}, val {:?}", pos, pos.to_val());
                midi.control_msg(0, 5, pos.to_val()).ok();
            }
        } else {
            iprint!(stim, "-");
            // delay(clocks.sysclk().0 / 10);
        }
    }
}
