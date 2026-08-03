//! USB AOA probe: enumerate Smartisan/accessory devices, dump interfaces,
//! try AOA control + bulk writes, and toggle interface alts to identify the
//! f_accessory data interface.
use rusb::{Context, Direction, UsbContext};

fn get_protocol(handle: &rusb::DeviceHandle<rusb::Context>, label: &str) {
    let mut proto = [0u8; 2];
    match handle.read_control(
        0xc0,
        0x51,
        0,
        0,
        &mut proto,
        std::time::Duration::from_millis(400),
    ) {
        Ok(n) => println!(
            "  {label} GET_PROTOCOL ok len={n} ver={}",
            u16::from_le_bytes(proto)
        ),
        Err(e) => println!("  {label} GET_PROTOCOL -> {e:?}"),
    }
}

fn main() {
    let context = Context::new().expect("libusb context");
    for device in context.devices().expect("devices").iter() {
        let descriptor = match device.device_descriptor() {
            Ok(d) => d,
            Err(e) => {
                println!("dev bus={} desc err {e}", device.bus_number());
                continue;
            }
        };
        let ports = device.port_numbers().unwrap_or_default();
        let location = format!(
            "{}-{}",
            device.bus_number(),
            ports
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join("-")
        );
        println!(
            "dev bus={} loc={} vid=0x{:04x} pid=0x{:04x} class=0x{:02x}",
            device.bus_number(),
            location,
            descriptor.vendor_id(),
            descriptor.product_id(),
            descriptor.class_code()
        );
        if false {
            continue;
        }
        let handle = match device.open() {
            Ok(h) => h,
            Err(e) => {
                println!("  open err {e:?}");
                continue;
            }
        };
        let _ = handle.set_auto_detach_kernel_driver(true);
        get_protocol(&handle, "before");

        // identification strings + ACCESSORY_START (mirror the Mac client
        // sendAOAStartupRequest: decimal request 0x34, zero-based indexes,
        // 1 ms spacing, GET_PROTOCOL failure tolerated).
        let strings: [(&str, u16); 5] = [
            ("Smartisan", 0),
            ("HandShaker", 1),
            ("HandShaker", 2),
            ("1.0", 3),
            ("e976ce6596c81fc5", 4),
        ];
        for (value, index) in strings {
            match handle.write_control(
                0x40,
                0x34,
                0,
                index,
                value.as_bytes(),
                std::time::Duration::from_secs(1),
            ) {
                Ok(n) => println!("  SEND_STRING[{index}] ok ({n}B)"),
                Err(e) => println!("  SEND_STRING[{index}] -> {e:?}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        match handle.write_control(0x40, 0x35, 0, 0, &[], std::time::Duration::from_secs(1)) {
            Ok(n) => println!("  ACCESSORY_START ok ({n})"),
            Err(e) => println!("  ACCESSORY_START -> {e:?}"),
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        // Re-enumerate after START.
        let mut saw_after = false;
        for d2 in context.devices().expect("devices").iter() {
            if let Ok(dd) = d2.device_descriptor()
                && (dd.vendor_id() == 0x18d1
                    || dd.vendor_id() == 0x29a9
                    || dd.vendor_id() == 0x05c6)
            {
                println!(
                    "  after START: vid=0x{:04x} pid=0x{:04x}",
                    dd.vendor_id(),
                    dd.product_id()
                );
                saw_after = true;
            }
        }
        if !saw_after {
            println!("  after START: (no smartisan/accessory device)");
        }

        let config = match device.active_config_descriptor() {
            Ok(c) => c,
            Err(e) => {
                println!("  config err {e:?}");
                continue;
            }
        };
        println!("  active config {} interfaces:", config.number());
        for iface in config.interfaces() {
            for alt in iface.descriptors() {
                let eps: Vec<String> = alt
                    .endpoint_descriptors()
                    .map(|e| {
                        format!(
                            "0x{:02x}{}{}",
                            e.address(),
                            if e.direction() == Direction::In {
                                "IN"
                            } else {
                                "OUT"
                            },
                            match e.transfer_type() {
                                rusb::TransferType::Bulk => "B",
                                rusb::TransferType::Interrupt => "I",
                                _ => "?",
                            }
                        )
                    })
                    .collect();
                println!(
                    "    iface {} alt {} class=0x{:02x} sub=0x{:02x} ep: {:?}",
                    iface.number(),
                    alt.interface_number(),
                    alt.class_code(),
                    alt.sub_class_code(),
                    eps
                );
            }
        }
        // Claim each vendor interface, try writing its OUT endpoint, and
        // toggle alts while probing GET_PROTOCOL.
        for iface in [0u8, 1, 3, 4] {
            if handle.claim_interface(iface).is_err() {
                println!("  iface {iface} claim failed");
                continue;
            }
            let mut buf = [0u8; 4096];
            match handle.read_bulk(0x83, &mut buf, std::time::Duration::from_millis(300)) {
                Ok(n) => println!(
                    "  iface {iface} read 0x83 got {n}B: {:02x?}",
                    &buf[..n.min(12)]
                ),
                Err(e) => println!("  iface {iface} read 0x83 -> {e:?}"),
            }
            let _ = handle.release_interface(iface);
        }
        break;
    }
}
