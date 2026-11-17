//! The device layer against the fake backend.

use std::sync::Arc;
use std::time::Duration;

use covalence_devices::fake::{Fake, esp32, stream_deck_mk2};
use covalence_devices::{
    DeviceError, DeviceEvent, Devices, HandleEvent, IoStateKind, OpenOptions, Opened, Settings,
};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(5);
const DECK: &str = "hid:0fd9:0080:AL12";

fn manual(fake: &Fake) -> Devices {
    Devices::new(Arc::new(fake.clone()), Settings::manual())
}

async fn next(events: &mut mpsc::Receiver<HandleEvent>) -> HandleEvent {
    tokio::time::timeout(WAIT, events.recv())
        .await
        .expect("an event in time")
        .expect("the handle is still there")
}

async fn state(events: &mut mpsc::Receiver<HandleEvent>) -> (IoStateKind, Option<DeviceError>) {
    loop {
        if let HandleEvent::State { state, error } = next(events).await {
            return (state, error);
        }
    }
}

async fn data(events: &mut mpsc::Receiver<HandleEvent>) -> Vec<u8> {
    loop {
        if let HandleEvent::Data { data, .. } = next(events).await {
            return data;
        }
    }
}

fn open(devices: &Devices, owner: &str, device: &str) -> Opened {
    devices
        .open(owner, device, OpenOptions::default())
        .expect("not busy")
}

#[tokio::test]
async fn lists_devices_by_stable_id_and_reports_changes() {
    let fake = Fake::new();
    let devices = manual(&fake);
    let mut changes = devices.subscribe();
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    fake.plug(esp32("/dev/ttyACM0", None));
    devices.scan();

    let ids: Vec<String> = devices.list().into_iter().map(|d| d.id).collect();
    assert_eq!(ids, [DECK, "serial@/dev/ttyACM0"]);
    for _ in 0..2 {
        assert!(matches!(changes.try_recv(), Ok(DeviceEvent::Added(_))));
    }

    // Another port, the same ID.
    fake.unplug("/dev/hidraw0");
    devices.scan();
    assert!(matches!(changes.try_recv(), Ok(DeviceEvent::Removed(d)) if d.id == DECK));
    fake.plug(stream_deck_mk2("/dev/hidraw7", Some("AL12")));
    devices.scan();
    assert!(
        matches!(changes.try_recv(), Ok(DeviceEvent::Added(d)) if d.id == DECK && d.path == "/dev/hidraw7")
    );
    devices.scan();
    assert!(changes.try_recv().is_err(), "nothing changed");
}

#[tokio::test]
async fn performs_a_handles_io() {
    let fake = Fake::new();
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    fake.set_feature("/dev/hidraw0", 0x06, &[0x06, 1, 2, 3, 4]);
    let devices = manual(&fake);
    devices.scan();

    let mut opened = open(&devices, "deck", DECK);
    assert!(!opened.reattached);
    assert_eq!(state(&mut opened.events).await, (IoStateKind::Open, None));

    fake.input("/dev/hidraw0", &[0x01, 0, 0, 0, 1]);
    assert_eq!(data(&mut opened.events).await, [0x01, 0, 0, 0, 1]);

    for n in 0..20u8 {
        devices.write(&opened.handle, vec![0x02, n]).unwrap();
    }
    let log = fake.wait_log("/dev/hidraw0", WAIT, |l| l.writes.len() == 20);
    let order: Vec<u8> = log.writes.iter().map(|w| w[1]).collect();
    assert_eq!(order, (0..20).collect::<Vec<_>>(), "writes stay in order");

    let got = devices.get_feature(&opened.handle, 0x06, 3).await.unwrap();
    assert_eq!(got, [0x06, 1, 2]);
    devices
        .send_feature(&opened.handle, vec![0x03, 0x08, 50])
        .await
        .unwrap();
    assert_eq!(
        fake.log("/dev/hidraw0").features_sent,
        [vec![0x03, 0x08, 50]]
    );

    devices.close(&opened.handle);
    assert_eq!(fake.open_ports("/dev/hidraw0"), 0);
    assert!(matches!(
        devices.write(&opened.handle, vec![0]),
        Err(DeviceError::NotOpen(_))
    ));
}

#[tokio::test]
async fn one_connection_holds_a_device() {
    let fake = Fake::new();
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    let devices = manual(&fake);
    devices.scan();

    let first = open(&devices, "a", DECK);
    let refused = devices.open("b", DECK, OpenOptions::default()).unwrap_err();
    assert_eq!(
        refused,
        DeviceError::Busy {
            device: DECK.into(),
            holder: "a".into()
        }
    );
    assert_eq!(refused.code(), "busy");
    assert_eq!(devices.holders().get(DECK).map(String::as_str), Some("a"));

    devices.release("a");
    assert!(devices.handles().is_empty());
    let mut second = open(&devices, "b", DECK);
    assert_ne!(second.handle, first.handle);
    assert_eq!(state(&mut second.events).await.0, IoStateKind::Open);
    assert_eq!(fake.open_ports("/dev/hidraw0"), 1);
}

#[tokio::test]
async fn unplug_and_replug_close_and_reopen_the_handle() {
    let fake = Fake::new();
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    let devices = manual(&fake);
    devices.scan();
    let mut opened = open(&devices, "deck", DECK);
    assert_eq!(state(&mut opened.events).await.0, IoStateKind::Open);

    fake.unplug("/dev/hidraw0");
    // The read fails before the next enumeration notices: still `unplugged`.
    assert_eq!(
        state(&mut opened.events).await,
        (IoStateKind::Closed, Some(DeviceError::Unplugged))
    );
    devices.scan();
    assert!(devices.write(&opened.handle, vec![1]).is_err());
    assert_eq!(fake.open_ports("/dev/hidraw0"), 0);

    // Back, in another port: the same handle opens by itself.
    fake.plug(stream_deck_mk2("/dev/hidraw4", Some("AL12")));
    devices.scan();
    assert_eq!(state(&mut opened.events).await, (IoStateKind::Open, None));
    devices.write(&opened.handle, vec![0x02, 9]).unwrap();
    let log = fake.wait_log("/dev/hidraw4", WAIT, |l| !l.writes.is_empty());
    assert_eq!(log.writes, [vec![0x02, 9]]);
    assert_eq!(devices.handles().len(), 1);
}

#[tokio::test]
async fn a_handle_outlives_its_holders_host() {
    let fake = Fake::new();
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    let devices = manual(&fake);
    devices.scan();
    let mut before = open(&devices, "deck", DECK);
    assert_eq!(state(&mut before.events).await.0, IoStateKind::Open);

    // The host goes away: input is dropped, the device stays open.
    devices.detach("deck");
    fake.input("/dev/hidraw0", &[0x01, 1]);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!devices.handles()[0].attached);

    // The restarted connection opens it again and gets the same handle.
    let mut after = open(&devices, "deck", DECK);
    assert!(after.reattached);
    assert_eq!(after.handle, before.handle);
    assert_eq!(state(&mut after.events).await, (IoStateKind::Open, None));
    fake.input("/dev/hidraw0", &[0x01, 2]);
    assert_eq!(
        data(&mut after.events).await,
        [0x01, 2],
        "the earlier input was dropped"
    );
    assert_eq!(fake.log("/dev/hidraw0").opens, 1, "never closed");
    assert!(before.events.try_recv().is_err());
}

#[tokio::test]
async fn a_device_that_is_not_there_opens_when_it_arrives() {
    let fake = Fake::new();
    let devices = manual(&fake);
    devices.scan();
    let mut opened = open(&devices, "esp", "serial:303a:1001:ESP1");
    assert_eq!(
        state(&mut opened.events).await,
        (IoStateKind::Closed, Some(DeviceError::Unplugged))
    );

    fake.plug(esp32("/dev/ttyACM0", Some("ESP1")));
    devices.scan();
    assert_eq!(state(&mut opened.events).await.0, IoStateKind::Open);
    fake.input("/dev/ttyACM0", b"hello\n");
    assert_eq!(data(&mut opened.events).await, b"hello\n");
    devices.write(&opened.handle, b"ping\n".to_vec()).unwrap();
    let log = fake.wait_log("/dev/ttyACM0", WAIT, |l| !l.writes.is_empty());
    assert_eq!(log.writes, [b"ping\n".to_vec()]);
    assert_eq!(log.baud, Some(covalence_devices::DEFAULT_BAUD));
    assert!(matches!(
        devices.get_feature(&opened.handle, 1, 8).await,
        Err(DeviceError::Unsupported(_))
    ));
}

#[tokio::test]
async fn os_refusals_are_explained_and_retried_on_replug() {
    let fake = Fake::new();
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    let denied = DeviceError::PermissionDenied("/dev/hidraw0: Permission denied".into());
    fake.fail_open("/dev/hidraw0", Some(denied.clone()));
    let devices = manual(&fake);
    devices.scan();

    let mut opened = open(&devices, "deck", DECK);
    let (kind, error) = state(&mut opened.events).await;
    assert_eq!((kind, error.clone()), (IoStateKind::Error, Some(denied)));
    let report = error.unwrap().report();
    assert_eq!(report.code, "permission_denied");
    assert!(report.hint.is_some());

    // Fixed (say, a udev rule added), but it waits for a replug.
    fake.fail_open("/dev/hidraw0", None);
    devices.scan();
    assert_eq!(fake.log("/dev/hidraw0").opens, 0);
    fake.unplug("/dev/hidraw0");
    devices.scan();
    assert_eq!(
        state(&mut opened.events).await.1,
        Some(DeviceError::Unplugged)
    );
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    devices.scan();
    assert_eq!(state(&mut opened.events).await, (IoStateKind::Open, None));
}

#[tokio::test]
async fn enumeration_errors_keep_what_was_there() {
    let fake = Fake::new();
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    let devices = manual(&fake);
    devices.scan();
    fake.fail_enumerate(Some(DeviceError::Io("udev is gone".into())));
    devices.scan();
    assert_eq!(devices.list().len(), 1);
    assert_eq!(
        devices.scan_error(),
        Some(DeviceError::Io("udev is gone".into()))
    );
    fake.fail_enumerate(None);
    devices.scan();
    assert_eq!(devices.scan_error(), None);
}

#[tokio::test]
async fn the_watcher_notices_hotplug_by_itself() {
    let fake = Fake::new();
    let devices = Devices::new(
        Arc::new(fake.clone()),
        Settings {
            // Long enough that only the hotplug notification can explain a
            // quick answer.
            poll: Duration::from_secs(60),
            ..Settings::default()
        },
    );
    let mut changes = devices.subscribe();
    tokio::time::sleep(Duration::from_millis(50)).await;
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    let added = tokio::time::timeout(WAIT, changes.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(added, DeviceEvent::Added(d) if d.id == DECK));

    let mut opened = open(&devices, "deck", DECK);
    assert_eq!(state(&mut opened.events).await.0, IoStateKind::Open);
    fake.unplug("/dev/hidraw0");
    let removed = tokio::time::timeout(WAIT, changes.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(removed, DeviceEvent::Removed(_)));
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    loop {
        if state(&mut opened.events).await.0 == IoStateKind::Open {
            break;
        }
    }
}
