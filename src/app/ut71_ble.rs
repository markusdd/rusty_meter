//! Scan and stream the UT-D07A. The frame decoder stays in `ut71x`.

impl super::MyApp {
    pub(super) fn start_ut71_ble_scan(&mut self) {
        if self.ut71_ble_scan_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.ut71_ble_scan_rx = Some(rx);
        tokio::spawn(async move {
            let _ = tx.send(crate::ut71_ble::scan().await);
        });
    }

    pub(super) fn poll_ut71_ble(&mut self, ctx: &egui::Context) {
        if self.connection_type == super::ConnectionType::UniTUt71Ble {
            if self.ut71_ble_scan_rx.is_none()
                && self.ut71_ble_devices.is_empty()
                && !self.ut71_ble_scanned
            {
                self.ut71_ble_scanned = true;
                self.start_ut71_ble_scan();
            }
        } else {
            self.ut71_ble_scanned = false;
        }
        if let Some(rx) = &self.ut71_ble_scan_rx {
            match rx.try_recv() {
                Ok(Ok(devices)) => {
                    self.ut71_ble_scan_rx = None;
                    let saved = self.ut71_ble_address.clone();
                    self.ut71_ble_devices = devices;
                    if !saved.is_empty()
                        && self
                            .ut71_ble_devices
                            .iter()
                            .any(|(addr, _)| addr.eq_ignore_ascii_case(&saved))
                    {
                        self.ut71_ble_address = saved;
                    } else if self.ut71_ble_devices.len() == 1 {
                        self.ut71_ble_address = self.ut71_ble_devices[0].0.clone();
                    }
                }
                Ok(Err(err)) => {
                    self.ut71_ble_scan_rx = None;
                    self.connection_error = Some(err);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200))
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ut71_ble_scan_rx = None;
                }
            }
        }
        if let Some(rx) = &self.ut71_ble_error_rx {
            match rx.try_recv() {
                Ok(err) => {
                    self.ut71_ble_error_rx = None;
                    if self.connection_state == super::ConnectionState::Connected
                        && self.connection_type == super::ConnectionType::UniTUt71Ble
                    {
                        self.disconnect();
                        self.connection_error = Some(err);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ut71_ble_error_rx = None;
                }
            }
        }
    }

    pub(super) fn spawn_ut71_ble_task(&mut self) {
        let address = self.ut71_ble_address.clone();
        let (tx_value, rx_value) = tokio::sync::mpsc::channel::<Option<f64>>(100);
        let (tx_mode, rx_mode) =
            tokio::sync::mpsc::channel::<(crate::multimeter::MeterMode, String)>(10);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (err_tx, err_rx) = std::sync::mpsc::channel();
        self.serial_rx = Some(rx_value);
        self.mode_rx = Some(rx_mode);
        self.serial_tx = None;
        self.shutdown_tx = Some(shutdown_tx);
        self.ut71_ble_error_rx = Some(err_rx);
        let debug = self.value_debug;
        tokio::spawn(async move {
            if let Err(err) =
                crate::ut71_ble::run(address, tx_value, tx_mode, shutdown_rx, debug).await
            {
                let _ = err_tx.send(err);
            }
        });
    }
}
