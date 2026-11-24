//! UNI-T UT-D07A Bluetooth adapter. It forwards the meter's 2400 7o1 bytes
//! as BLE notifications, one or a few bytes at a time, with the parity bit
//! still set in bit 7.

use std::time::Duration;

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::multimeter::MeterMode;
use crate::ut71x;

const SERVICE_UUID: Uuid = Uuid::from_u128(0x49535343_fe7d_4ae5_8fa9_9fafd205e455);
const NOTIFY_UUID: Uuid = Uuid::from_u128(0x49535343_1e4d_4bd9_ba61_23c647249616);

pub async fn scan() -> Result<Vec<(String, String)>, String> {
    let adapter = first_adapter().await?;
    adapter
        .start_scan(ScanFilter {
            services: vec![SERVICE_UUID],
        })
        .await
        .map_err(|err| err.to_string())?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mut found = Vec::new();
    for peripheral in adapter.peripherals().await.map_err(|err| err.to_string())? {
        let Some(props) = peripheral
            .properties()
            .await
            .map_err(|err| err.to_string())?
        else {
            continue;
        };
        let name = props.local_name.unwrap_or_default();
        let advertised = props.services.contains(&SERVICE_UUID);
        let looks_like_adapter = {
            let upper = name.to_ascii_uppercase();
            upper.contains("D07") || upper.contains("UT71") || upper.contains("UNI-T")
        };
        if !advertised && !looks_like_adapter {
            continue;
        }
        let address = props.address.to_string();
        let label = if name.is_empty() {
            address.clone()
        } else {
            format!("{name} ({address})")
        };
        found.push((address, label));
    }
    let _ = adapter.stop_scan().await;
    found.sort_by_key(|device| device.1.to_ascii_lowercase());
    found.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    Ok(found)
}

pub async fn run(
    address: String,
    tx_value: mpsc::Sender<Option<f64>>,
    tx_mode: mpsc::Sender<(MeterMode, String)>,
    mut shutdown: oneshot::Receiver<()>,
    debug: bool,
) -> Result<(), String> {
    let adapter = first_adapter().await?;
    adapter
        .start_scan(ScanFilter {
            services: vec![SERVICE_UUID],
        })
        .await
        .map_err(|err| err.to_string())?;
    let peripheral = find_peripheral(&adapter, &address).await?;
    let _ = adapter.stop_scan().await;
    peripheral.connect().await.map_err(|err| err.to_string())?;
    peripheral
        .discover_services()
        .await
        .map_err(|err| err.to_string())?;
    let characteristic = peripheral
        .characteristics()
        .into_iter()
        .find(|chr| chr.uuid == NOTIFY_UUID)
        .ok_or_else(|| "UT-D07A data characteristic was not found".to_owned())?;
    peripheral
        .subscribe(&characteristic)
        .await
        .map_err(|err| err.to_string())?;
    let mut notes = peripheral
        .notifications()
        .await
        .map_err(|err| err.to_string())?;
    let mut packet_buf = Vec::new();
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            note = notes.next() => {
                let Some(note) = note else { break };
                if note.uuid != NOTIFY_UUID {
                    continue;
                }
                for reading in ut71x::feed_ble_bytes(&mut packet_buf, &note.value) {
                    if debug {
                        println!(
                            "UT71 BLE: {} {:?} {}",
                            reading.value, reading.mode, reading.unit
                        );
                    }
                    if tx_value.send(Some(reading.value)).await.is_err() {
                        break;
                    }
                    let _ = tx_mode.send((reading.mode, reading.unit)).await;
                }
            }
        }
    }
    let _ = peripheral.disconnect().await;
    Ok(())
}

async fn first_adapter() -> Result<btleplug::platform::Adapter, String> {
    let manager = Manager::new().await.map_err(|err| err.to_string())?;
    let mut adapters = manager.adapters().await.map_err(|err| err.to_string())?;
    adapters
        .pop()
        .ok_or_else(|| "No Bluetooth adapter found".to_owned())
}

async fn find_peripheral(
    adapter: &btleplug::platform::Adapter,
    address: &str,
) -> Result<btleplug::platform::Peripheral, String> {
    let want = normalize_address(address);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        for peripheral in adapter.peripherals().await.map_err(|err| err.to_string())? {
            let Some(props) = peripheral
                .properties()
                .await
                .map_err(|err| err.to_string())?
            else {
                continue;
            };
            if normalize_address(&props.address.to_string()) == want {
                return Ok(peripheral);
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(format!("Could not find Bluetooth adapter {address}"))
}

fn normalize_address(address: &str) -> String {
    address
        .chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}
