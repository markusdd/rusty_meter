use std::ffi::CString;
use std::time::Duration;

use hidapi::HidApi;
use tokio::sync::{mpsc, oneshot};

use crate::multimeter::MeterMode;
use crate::victor::{self, VICTOR_PRODUCT_ID, VICTOR_VENDOR_ID};

impl super::MyApp {
    pub fn refresh_hid_devices(&mut self) {
        self.hid_devicelist.clear();
        match HidApi::new() {
            Ok(api) => {
                for device in api.device_list() {
                    if device.vendor_id() == VICTOR_VENDOR_ID
                        && device.product_id() == VICTOR_PRODUCT_ID
                    {
                        let path = device.path().to_string_lossy().into_owned();
                        let label = format!(
                            "{} {} ({:04x}:{:04x})",
                            device.manufacturer_string().unwrap_or("Victor"),
                            device.product_string().unwrap_or("Multimeter"),
                            device.vendor_id(),
                            device.product_id(),
                        );
                        self.hid_devicelist.push_back((path, label));
                    }
                }
            }
            Err(e) => {
                if self.value_debug {
                    println!("Failed to enumerate HID devices: {}", e);
                }
            }
        }
        if self.hid_device_path.is_empty() {
            if let Some((path, _)) = self.hid_devicelist.front() {
                self.hid_device_path = path.clone();
            }
        }
    }

    pub fn spawn_hid_task(&mut self) {
        let (tx_data, rx_data) = mpsc::channel::<Option<f64>>(100);
        let (tx_mode, rx_mode) = mpsc::channel::<(MeterMode, String)>(10);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        self.serial_rx = Some(rx_data);
        self.mode_rx = Some(rx_mode);
        self.shutdown_tx = Some(shutdown_tx);
        // Victor meters are read-only; no command channel.
        self.serial_tx = None;

        let device_path = self.hid_device_path.clone();
        let value_debug_shared = self.value_debug_shared.clone();
        let poll_interval_shared = self.poll_interval_shared.clone();
        let device_shared = self.device.clone();

        tokio::task::spawn_blocking(move || {
            let api = match HidApi::new() {
                Ok(api) => api,
                Err(e) => {
                    if *value_debug_shared.lock().unwrap() {
                        println!("Failed to create HID API: {}", e);
                    }
                    return;
                }
            };

            let c_path = match CString::new(device_path.as_bytes()) {
                Ok(path) => path,
                Err(e) => {
                    if *value_debug_shared.lock().unwrap() {
                        println!("Invalid Victor HID device path: {}", e);
                    }
                    return;
                }
            };
            let device = match api.open_path(&c_path) {
                Ok(device) => device,
                Err(e) => {
                    if *value_debug_shared.lock().unwrap() {
                        println!("Failed to open Victor HID device: {}", e);
                    }
                    return;
                }
            };

            {
                let mut dev = device_shared.lock().unwrap();
                *dev = "Victor 86 series (read only)".to_owned();
            }

            if *value_debug_shared.lock().unwrap() {
                println!("Victor HID device opened");
            }

            let mut readbuf = [0u8; 64];
            let mut shutting_down = false;
            let mut last_mode = None::<MeterMode>;

            loop {
                if shutdown_rx.try_recv().is_ok() {
                    shutting_down = true;
                }
                if shutting_down {
                    break;
                }

                let interval = *poll_interval_shared.lock().unwrap();
                let timeout_ms = interval.max(50) as i32;

                match device.read_timeout(&mut readbuf, timeout_ms) {
                    Ok(0) => continue,
                    Ok(len) => {
                        if *value_debug_shared.lock().unwrap() {
                            println!("Victor HID received {} bytes", len);
                        }
                        if let Some(reading) = victor::parse_hid_buffer(&readbuf[..len]) {
                            if *value_debug_shared.lock().unwrap() {
                                println!("Victor reading: {} {:?}", reading.value, reading.mode);
                            }
                            let _ = tx_data.blocking_send(Some(reading.value));
                            if last_mode != Some(reading.mode) {
                                last_mode = Some(reading.mode);
                                let _ = tx_mode.blocking_send((reading.mode, reading.unit));
                            }
                        }
                    }
                    Err(e) => {
                        if *value_debug_shared.lock().unwrap() {
                            println!("Victor HID read error: {}", e);
                        }
                        std::thread::sleep(Duration::from_millis(interval));
                    }
                }
            }

            if *value_debug_shared.lock().unwrap() {
                println!("Victor HID task shutting down");
            }
        });
    }
}
