//! Unit and hardware-assisted regression coverage for performance sources.

use super::*;
use idevice::IdeviceService;
use idevice::core_device_proxy::CoreDeviceProxy;
use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};

#[test]
fn aggregate_cpu_load_is_normalized_by_device_cpu_count() {
    assert_eq!(normalize_aggregate_cpu_percent(240.0, 6), Some(40.0));
    assert_eq!(normalize_aggregate_cpu_percent(600.0, 6), Some(100.0));
    assert_eq!(normalize_aggregate_cpu_percent(601.0, 6), None);
    assert_eq!(normalize_aggregate_cpu_percent(42.0, 0), None);
    assert_eq!(normalize_aggregate_cpu_percent(f64::NAN, 6), None);
}

#[test]
fn hardware_metrics_are_bounded_and_logical_count_falls_back() {
    let mut hardware = plist::Dictionary::new();
    hardware.insert("numberOfPhysicalCpus".into(), Value::Integer(6.into()));
    hardware.insert(
        "physicalMemory".into(),
        Value::Integer(6_442_450_944_u64.into()),
    );
    assert_eq!(cpu_count(&hardware), Some(6));
    assert_eq!(physical_cpu_count(&hardware), Some(6));
    assert_eq!(physical_memory_bytes(&hardware), Some(6_442_450_944));

    hardware.insert("numberOfCpus".into(), Value::Integer(8.into()));
    assert_eq!(cpu_count(&hardware), Some(8));

    hardware.insert("numberOfCpus".into(), Value::Integer(0.into()));
    assert_eq!(cpu_count(&hardware), Some(6));

    hardware.insert("numberOfPhysicalCpus".into(), Value::Integer(257.into()));
    hardware.insert("physicalMemory".into(), Value::Integer(u64::MAX.into()));
    assert_eq!(cpu_count(&hardware), None);
    assert_eq!(physical_cpu_count(&hardware), None);
    assert_eq!(physical_memory_bytes(&hardware), None);
}

#[test]
fn hardware_metrics_are_retained_across_partial_samples() {
    let slot = PerformanceSlot::default();
    let hardware = plist::Dictionary::from_iter([
        (String::from("numberOfCpus"), Value::Integer(8.into())),
        (
            String::from("numberOfPhysicalCpus"),
            Value::Integer(6.into()),
        ),
        (
            String::from("physicalMemory"),
            Value::Integer(6_442_450_944_u64.into()),
        ),
    ]);
    update_hardware(&slot, &hardware);
    update_graphics(
        &slot,
        &idevice::dvt::graphics::GraphicsSample {
            timestamp: 1,
            fps: 60.0,
            alloc_system_memory: 10,
            in_use_system_memory: 8,
            in_use_system_memory_driver: 3,
            gpu_bundle_name: "Built-In".into(),
            recovery_count: 0,
        },
    );
    let snapshot = slot.get();
    assert_eq!(snapshot.logical_cpu_count, Some(8));
    assert_eq!(snapshot.physical_cpu_count, Some(6));
    assert_eq!(snapshot.physical_memory_bytes, Some(6_442_450_944));
    let serialized = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(serialized["physical_cpu_count"], 6);
    assert_eq!(serialized["physical_memory_bytes"], 6_442_450_944_u64);
}

#[test]
fn network_interface_catalog_is_classified_sanitized_and_bounded() {
    let mut network = plist::Dictionary::from_iter([
        (String::from("en0"), Value::String("  Wi-Fi  ".into())),
        (
            String::from("pdp_ip0"),
            Value::String("Cellular (pdp_ip0)".into()),
        ),
        (
            String::from("en2"),
            Value::String("Ethernet\nAdaptor (en2)".into()),
        ),
        (String::from("lo0"), Value::String("Loopback".into())),
        (String::from("utun0"), Value::String("Tunnel".into())),
        (String::from("bad/name"), Value::String("Private".into())),
        (String::from("empty"), Value::String("   ".into())),
        (String::from("control"), Value::String("bad\0value".into())),
        (String::from("numeric"), Value::Integer(1.into())),
    ]);
    let (interfaces, truncated) = normalize_network_interfaces(&network);
    assert!(!truncated);
    assert_eq!(interfaces.len(), 5);
    assert_eq!(interfaces[0].name, "en0");
    assert_eq!(interfaces[0].kind, DeviceNetworkInterfaceKind::Wifi);
    assert_eq!(interfaces[1].kind, DeviceNetworkInterfaceKind::Cellular);
    assert_eq!(interfaces[2].description, "Ethernet Adaptor (en2)");
    assert_eq!(interfaces[3].kind, DeviceNetworkInterfaceKind::Loopback);
    assert_eq!(interfaces[4].kind, DeviceNetworkInterfaceKind::Other);

    network.clear();
    for index in 0..=MAX_NETWORK_INTERFACES {
        network.insert(format!("utun{index}"), Value::String("Tunnel".into()));
    }
    let (interfaces, truncated) = normalize_network_interfaces(&network);
    assert_eq!(interfaces.len(), MAX_NETWORK_INTERFACES);
    assert!(truncated);
}

#[test]
fn network_interface_catalog_is_retained_and_serialized_without_addresses() {
    let slot = PerformanceSlot::default();
    update_network_interfaces(
        &slot,
        &plist::Dictionary::from_iter([(String::from("en0"), Value::String("Wi-Fi".into()))]),
    );
    update_graphics(
        &slot,
        &idevice::dvt::graphics::GraphicsSample {
            timestamp: 1,
            fps: 60.0,
            alloc_system_memory: 10,
            in_use_system_memory: 8,
            in_use_system_memory_driver: 3,
            gpu_bundle_name: "Built-In".into(),
            recovery_count: 0,
        },
    );
    let serialized = serde_json::to_value(slot.get()).unwrap();
    assert_eq!(serialized["network_interfaces"][0]["name"], "en0");
    assert_eq!(serialized["network_interfaces"][0]["kind"], "wifi");
    assert_eq!(serialized["network_interfaces"][0]["description"], "Wi-Fi");
    assert_eq!(serialized["network_interfaces_available"], true);
    assert_eq!(serialized["network_interfaces_truncated"], false);
    assert!(serialized.get("address").is_none());
}

#[test]
fn partial_system_samples_preserve_the_latest_metrics() {
    let slot = PerformanceSlot::default();
    let mut cpu = plist::Dictionary::new();
    cpu.insert("CPU_TotalLoad".into(), Value::Real(240.0));
    let mut processes = plist::Dictionary::new();
    processes.insert("1".into(), Value::Array(Vec::new()));
    update_system(
        &slot,
        &SysmontapSample {
            processes: Some(processes),
            system: None,
            system_cpu_usage: Some(cpu),
        },
        6,
        &ProcessSchema::default(),
    );
    update_system(
        &slot,
        &SysmontapSample {
            processes: None,
            system: None,
            system_cpu_usage: None,
        },
        6,
        &ProcessSchema::default(),
    );

    let snapshot = slot.get();
    assert_eq!(snapshot.system_cpu_percent, Some(40.0));
    assert_eq!(snapshot.process_count, Some(1));
    assert_eq!(snapshot.logical_cpu_count, Some(6));
    assert_eq!(snapshot.top_processes.len(), 1);
    assert_eq!(snapshot.top_processes[0].pid, 1);
    assert_eq!(snapshot.top_processes[0].name, "pid 1");
}

#[test]
fn process_metrics_follow_the_negotiated_attribute_order() {
    let attributes = vec![
        "physFootprint".into(),
        "name".into(),
        "cpuUsage".into(),
        "pid".into(),
    ];
    let schema = ProcessSchema::new(&attributes);
    let row = Value::Array(vec![
        Value::Integer(25_000_000.into()),
        Value::String("Example\nGame".into()),
        Value::Real(120.0),
        Value::Integer(42.into()),
    ]);
    let process = normalize_process("ignored", &row, &schema, 6).unwrap();
    assert_eq!(process.pid, 42);
    assert_eq!(process.name, "ExampleGame");
    assert_eq!(process.cpu_percent, Some(20.0));
    assert_eq!(process.memory_bytes, Some(25_000_000));
}

#[test]
fn top_processes_include_cpu_and_memory_leaders() {
    let attributes = vec![
        "pid".into(),
        "name".into(),
        "cpuUsage".into(),
        "physFootprint".into(),
    ];
    let schema = ProcessSchema::new(&attributes);
    let mut processes = plist::Dictionary::new();
    for pid in 1..=12_u32 {
        processes.insert(
            pid.to_string(),
            Value::Array(vec![
                Value::Integer(pid.into()),
                Value::String(format!("cpu-{pid}")),
                Value::Real(f64::from(100 - pid)),
                Value::Integer(u64::from(pid * 1_000).into()),
            ]),
        );
    }
    processes.insert(
        "99".into(),
        Value::Array(vec![
            Value::Integer(99.into()),
            Value::String("memory-leader".into()),
            Value::Real(0.0),
            Value::Integer(9_000_000_000_u64.into()),
        ]),
    );

    let top = top_processes(&processes, &schema, 6);
    assert!(top.len() <= TOP_PROCESSES_PER_METRIC * 2);
    assert!(top.iter().any(|process| process.pid == 1));
    assert!(top.iter().any(|process| process.pid == 99));
    assert_eq!(top[0].pid, 1);
}

#[test]
fn performance_slot_merges_independent_sources() {
    let slot = PerformanceSlot::default();
    update_graphics(
        &slot,
        &idevice::dvt::graphics::GraphicsSample {
            timestamp: 1,
            fps: 59.5,
            alloc_system_memory: 10,
            in_use_system_memory: 8,
            in_use_system_memory_driver: 3,
            gpu_bundle_name: "Built-In".into(),
            recovery_count: 0,
        },
    );
    let snapshot = slot.get();
    assert_eq!(snapshot.graphics_fps, Some(59.5));
    assert_eq!(snapshot.gpu_in_use_bytes, Some(8));
    assert!(snapshot.captured_at_ms > 0);
}

#[test]
fn app_activity_is_sanitized_bounded_and_reset_with_the_session() {
    let slot = PerformanceSlot::default();
    for index in 0..=MAX_ACTIVITY_EVENTS {
        publish_app_activity(
            &slot,
            NotificationInfo {
                notification_type: " application\nstate ".into(),
                mach_absolute_time: index as i64,
                exec_name: " Example\tGame ".into(),
                app_name: " Example  Game ".into(),
                pid: (index + 1) as u32,
                state_description: " foreground\nactive ".into(),
            },
        );
    }

    let events = slot.app_activity();
    assert_eq!(events.len(), MAX_ACTIVITY_EVENTS);
    assert_eq!(events.first().unwrap().sequence, 2);
    assert_eq!(events.last().unwrap().sequence, 101);
    assert_eq!(
        events.last().unwrap().notification_type,
        "application state"
    );
    assert_eq!(
        events.last().unwrap().exec_name.as_deref(),
        Some("Example Game")
    );
    assert_eq!(
        events.last().unwrap().app_name.as_deref(),
        Some("Example Game")
    );
    assert_eq!(
        events.last().unwrap().state_description.as_deref(),
        Some("foreground active")
    );
    assert_eq!(events.last().unwrap().pid, Some(101));

    slot.reset();
    assert!(slot.app_activity().is_empty());
}

#[test]
fn energy_sampling_tracks_ranked_processes_and_sanitizes_scores() {
    let slot = PerformanceSlot::default();
    slot.observe(devicehub_core::PerformanceObservation::System {
        logical_cpu_count: 1,
        processes: Some(devicehub_core::ProcessPerformanceObservation {
            process_count: 20,
            top_processes: (0..20)
                .map(|index| ProcessPerformance {
                    pid: 100 - index,
                    name: format!("rank-{index}"),
                    cpu_percent: Some(f64::from(20 - index)),
                    memory_bytes: None,
                })
                .collect(),
        }),
        system_cpu_percent: None,
    });
    let targets = slot.energy_targets();
    assert_eq!(targets.len(), MAX_ENERGY_PROCESSES);
    assert!(targets.contains(&100));
    assert!(targets.contains(&85));
    assert!(!targets.contains(&84));

    update_energy(
        &slot,
        vec![
            EnergySample {
                pid: 100,
                timestamp: 1,
                total_energy: 5.0,
                cpu_energy: f64::NAN,
                gpu_energy: -2.0,
                networking_energy: 1.5,
                display_energy: 0.5,
                location_energy: 0.0,
                appstate_energy: f64::INFINITY,
            },
            EnergySample {
                pid: 99,
                timestamp: 1,
                total_energy: 8.0,
                cpu_energy: 3.0,
                gpu_energy: 2.0,
                networking_energy: 1.0,
                display_energy: 1.0,
                location_energy: 0.5,
                appstate_energy: 0.5,
            },
            EnergySample {
                pid: 777,
                timestamp: 1,
                total_energy: 99.0,
                cpu_energy: 99.0,
                gpu_energy: 0.0,
                networking_energy: 0.0,
                display_energy: 0.0,
                location_energy: 0.0,
                appstate_energy: 0.0,
            },
        ],
    );
    let snapshot = slot.get();
    assert_eq!(snapshot.energy_processes.len(), 2);
    assert_eq!(snapshot.energy_processes[0].pid, 99);
    assert_eq!(snapshot.energy_processes[0].name, "rank-1");
    assert_eq!(snapshot.energy_processes[1].cpu_score, 0.0);
    assert_eq!(snapshot.energy_processes[1].gpu_score, 0.0);
    assert_eq!(snapshot.energy_processes[1].app_state_score, 0.0);
}

#[test]
fn network_rates_use_connection_deltas_and_expire_stale_entries() {
    use idevice::dvt::network_monitor::{ConnectionDetectionEvent, ConnectionUpdateEvent};

    let started = Instant::now();
    let mut accumulator = NetworkAccumulator::new(started);
    accumulator.observe(
        NetworkEvent::ConnectionDetection(ConnectionDetectionEvent {
            local_address: None,
            remote_address: None,
            interface_index: 1,
            pid: 42,
            recv_buffer_size: 0,
            recv_buffer_used: 0,
            serial_number: 7,
            kind: 0,
        }),
        started,
    );
    accumulator.observe(
        NetworkEvent::ConnectionUpdate(ConnectionUpdateEvent {
            rx_packets: 1,
            rx_bytes: 1_000,
            tx_packets: 1,
            tx_bytes: 200,
            rx_dups: 0,
            rx_ooo: 0,
            tx_retx: 0,
            min_rtt: 0,
            avg_rtt: 0,
            connection_serial: 7,
            time: 0,
        }),
        started + Duration::from_millis(500),
    );
    let first = accumulator.sample(started + Duration::from_secs(1));
    assert_eq!(first.rx_bytes_per_second, 0.0);
    assert_eq!(first.tx_bytes_per_second, 0.0);
    assert_eq!(first.recent_connections, 1);

    accumulator.observe(
        NetworkEvent::ConnectionUpdate(ConnectionUpdateEvent {
            rx_packets: 2,
            rx_bytes: 1_500,
            tx_packets: 2,
            tx_bytes: 500,
            rx_dups: 0,
            rx_ooo: 0,
            tx_retx: 0,
            min_rtt: 0,
            avg_rtt: 0,
            connection_serial: 7,
            time: 1,
        }),
        started + Duration::from_millis(1_500),
    );
    let second = accumulator.sample(started + Duration::from_secs(2));
    assert_eq!(second.rx_bytes_per_second, 500.0);
    assert_eq!(second.tx_bytes_per_second, 300.0);
    assert_eq!(
        accumulator
            .sample(started + NETWORK_CONNECTION_TTL + Duration::from_secs(2))
            .recent_connections,
        0
    );
}

#[tokio::test]
#[ignore = "requires a connected physical device"]
async fn inspects_sysmontap_process_schema_from_hardware() {
    let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
    let device = usbmuxd
        .get_devices()
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("no connected device");
    let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-performance-test");
    let proxy = CoreDeviceProxy::connect(&provider).await.unwrap();
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let adapter = proxy.create_software_tunnel().unwrap();
    let mut adapter = adapter.to_async_handle();
    let stream = adapter.connect(rsd_port).await.unwrap();
    let mut handshake = RsdHandshake::new(stream).await.unwrap();
    let mut remote = RemoteServerClient::connect_rsd(&mut adapter, &mut handshake)
        .await
        .unwrap();
    let (process_attributes, system_attributes, logical_cpu_count) = {
        let mut info = DeviceInfoClient::new(&mut remote).await.unwrap();
        (
            info.sysmon_process_attributes().await.unwrap(),
            info.sysmon_system_attributes().await.unwrap(),
            cpu_count(&info.hardware_information().await.unwrap()).unwrap(),
        )
    };
    let process_schema = ProcessSchema::new(&process_attributes);
    assert!(process_schema.has_expected_fields());
    let mut client = SysmontapClient::new(&mut remote).await.unwrap();
    client
        .set_config(&SysmontapConfig {
            interval_ms: SAMPLE_INTERVAL_MS,
            process_attributes: process_attributes.clone(),
            system_attributes,
        })
        .await
        .unwrap();
    client.start().await.unwrap();
    let processes = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(processes) = client.next_sample().await.unwrap().processes {
                break processes;
            }
        }
    })
    .await
    .expect("timed out waiting for process sample");
    let top = top_processes(&processes, &process_schema, logical_cpu_count);
    assert!(!top.is_empty());
    assert!(top.iter().all(|process| !process.name.is_empty()));
    assert!(
        top.iter()
            .filter_map(|process| process.cpu_percent)
            .all(|cpu| (0.0..=100.0).contains(&cpu))
    );
    assert!(top.iter().any(|process| process.memory_bytes.is_some()));
    println!("normalized top processes: {:#?}", &top[..top.len().min(5)]);
    client.stop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a connected physical device"]
async fn receives_network_monitor_event_from_hardware() {
    let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
    let device = usbmuxd
        .get_devices()
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("no connected device");
    let provider = device.to_provider(
        UsbmuxdAddr::default(),
        "devicehub-mask-network-monitor-test",
    );
    let proxy = CoreDeviceProxy::connect(&provider).await.unwrap();
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let adapter = proxy.create_software_tunnel().unwrap();
    let mut adapter = adapter.to_async_handle();
    let stream = adapter.connect(rsd_port).await.unwrap();
    let mut handshake = RsdHandshake::new(stream).await.unwrap();
    let mut remote = RemoteServerClient::connect_rsd(&mut adapter, &mut handshake)
        .await
        .unwrap();
    let mut client = NetworkMonitorClient::new(&mut remote).await.unwrap();
    client.start_monitoring().await.unwrap();
    let (serial, rx_delta, tx_delta, detections, updates) =
        tokio::time::timeout(Duration::from_secs(20), async {
            let mut detections = 0_u32;
            let mut updates = 0_u32;
            let mut baselines = HashMap::<u64, (u64, u64)>::new();
            loop {
                let event = client.next_event().await.unwrap();
                match event {
                    NetworkEvent::ConnectionDetection(_) => detections += 1,
                    NetworkEvent::ConnectionUpdate(update) => {
                        updates += 1;
                        if let Some((previous_rx, previous_tx)) = baselines
                            .insert(update.connection_serial, (update.rx_bytes, update.tx_bytes))
                        {
                            let rx_delta = update.rx_bytes.saturating_sub(previous_rx);
                            let tx_delta = update.tx_bytes.saturating_sub(previous_tx);
                            if rx_delta > 0 || tx_delta > 0 {
                                break (
                                    update.connection_serial,
                                    rx_delta,
                                    tx_delta,
                                    detections,
                                    updates,
                                );
                            }
                        }
                    }
                    NetworkEvent::InterfaceDetection(_) | NetworkEvent::Unknown(_) => {}
                }
            }
        })
        .await
        .expect("timed out waiting for a positive network counter delta");
    println!(
        "received network delta for serial {serial} after {detections} detections and {updates} updates: rx={rx_delta} tx={tx_delta}"
    );
    client.stop_monitoring().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a connected physical device"]
async fn receives_energy_sample_from_hardware() {
    let mut usbmuxd = UsbmuxdConnection::default().await.unwrap();
    let device = usbmuxd
        .get_devices()
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("no connected device");
    let provider = device.to_provider(UsbmuxdAddr::default(), "devicehub-mask-energy-monitor-test");
    let proxy = CoreDeviceProxy::connect(&provider).await.unwrap();
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let adapter = proxy.create_software_tunnel().unwrap();
    let mut adapter = adapter.to_async_handle();
    let stream = adapter.connect(rsd_port).await.unwrap();
    let mut handshake = RsdHandshake::new(stream).await.unwrap();
    let mut energy_adapter = adapter.clone();
    let mut energy_handshake = handshake.clone();
    let mut remote = RemoteServerClient::connect_rsd(&mut adapter, &mut handshake)
        .await
        .unwrap();
    let process = {
        let mut info = DeviceInfoClient::new(&mut remote).await.unwrap();
        let processes = info.running_processes().await.unwrap();
        processes
            .iter()
            .find(|process| process.is_application && process.pid > 0)
            .or_else(|| processes.iter().find(|process| process.pid > 1))
            .cloned()
            .expect("no running process found")
    };
    drop(remote);

    let mut energy_remote =
        RemoteServerClient::connect_rsd(&mut energy_adapter, &mut energy_handshake)
            .await
            .unwrap();
    let mut client = EnergyMonitorClient::new(&mut energy_remote).await.unwrap();
    client.start_sampling(&[process.pid]).await.unwrap();
    let observations = tokio::time::timeout(Duration::from_secs(10), async {
        let mut observations = Vec::new();
        while observations.len() < 3 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let bytes = client.sample_attributes(&[process.pid]).await.unwrap();
            let samples = EnergySample::from_bytes(&bytes).unwrap();
            if let Some(sample) = samples.into_iter().find(|sample| {
                sample.pid == process.pid && (sample.timestamp > 0 || sample.total_energy > 0.0)
            }) && observations
                .last()
                .is_none_or(|previous: &EnergySample| sample.timestamp > previous.timestamp)
            {
                observations.push(sample);
            }
        }
        observations
    })
    .await
    .expect("energy sample timestamp did not advance");
    assert!(observations.iter().all(|sample| sample.pid == process.pid));
    assert!(
        observations
            .iter()
            .all(|sample| sample.total_energy.is_finite())
    );
    assert!(
        observations
            .windows(2)
            .all(|samples| samples[1].timestamp > samples[0].timestamp)
    );
    println!("received energy samples for {process:?}: {observations:#?}");
    client.stop_sampling(&[process.pid]).await.unwrap();
}
