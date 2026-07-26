use idevice::usbmuxd::{Connection, UsbmuxdConnection, UsbmuxdDevice};

pub(crate) async fn usb_test_device() -> UsbmuxdDevice {
    let Some(expected_udid) = option_env!("DEVICEHUB_TEST_UDID") else {
        panic!("DEVICEHUB_TEST_UDID must identify the USB test device");
    };
    assert!(
        !expected_udid.trim().is_empty(),
        "test device UDID is empty"
    );

    let mut usbmuxd = UsbmuxdConnection::default()
        .await
        .expect("connect to usbmuxd");
    usbmuxd
        .get_devices()
        .await
        .expect("list usbmuxd devices")
        .into_iter()
        .find(|device| {
            device.udid == expected_udid && matches!(device.connection_type, Connection::Usb)
        })
        .expect("requested USB test device is not connected")
}
