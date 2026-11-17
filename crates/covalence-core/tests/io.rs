//! The core's TCP transport: framing, reconnects and closing.

use std::time::Duration;

use covalence_core::io::IoHub;
use covalence_protocol::io::{Backoff, Framing, IoClose, IoOpen, IoOptions, IoWrite};
use covalence_protocol::{Bytes, Message, Value, from_value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(5);

/// The next `io.data` or `io.state` sent to the host, as `(method, params)`.
async fn next(out: &mut mpsc::UnboundedReceiver<Message>) -> (String, Value) {
    let message = tokio::time::timeout(WAIT, out.recv())
        .await
        .expect("a message in time")
        .expect("the hub is alive");
    let Message::Notification(n) = message else {
        panic!("{message:?}")
    };
    assert_eq!(n.conn.as_deref(), Some("c1"));
    (n.m, n.p)
}

async fn data(out: &mut mpsc::UnboundedReceiver<Message>) -> Vec<u8> {
    loop {
        let (m, p) = next(out).await;
        if m == "io.data" {
            let d: covalence_protocol::io::IoData = from_value(&p).unwrap();
            return d.data.0;
        }
    }
}

async fn state(out: &mut mpsc::UnboundedReceiver<Message>) -> covalence_protocol::io::IoState {
    loop {
        let (m, p) = next(out).await;
        if m == "io.state" {
            return from_value(&p).unwrap();
        }
    }
}

fn tcp(port: u16, options: IoOptions) -> IoOpen {
    IoOpen {
        kind: covalence_protocol::io::IoKind::Tcp,
        target: format!("127.0.0.1:{port}"),
        options: Some(options),
    }
}

#[tokio::test]
async fn frames_lines_and_lengths() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut first = [0u8; 1];
            socket.read_exact(&mut first).await.unwrap();
            if first[0] == b'L' {
                socket.write_all(b"one\ntw").await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                socket.write_all(b"o\nthree").await.unwrap();
            } else {
                socket.write_all(&[0, 0, 0, 3, b'a', b'b']).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                socket.write_all(&[b'c', 0, 0, 0, 0]).await.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    let hub = IoHub::new();
    let (tx, mut out) = mpsc::unbounded_channel();
    hub.attach(tx);

    let options = IoOptions {
        framing: Some(Framing::Line),
        ..IoOptions::default()
    };
    let (opened, start) = hub.open("c1", tcp(port, options)).await.unwrap();
    start.go();
    assert_eq!(
        state(&mut out).await.state,
        covalence_protocol::io::IoStateKind::Open
    );
    hub.write(
        "c1",
        IoWrite {
            handle: opened.handle.clone(),
            data: Bytes(b"L".to_vec()),
        },
    );
    assert_eq!(data(&mut out).await, b"one\n");
    assert_eq!(data(&mut out).await, b"two\n");
    hub.close(
        "c1",
        IoClose {
            handle: opened.handle,
        },
    );

    let options = IoOptions {
        framing: Some(Framing::Length),
        ..IoOptions::default()
    };
    let (opened, start) = hub.open("c1", tcp(port, options)).await.unwrap();
    start.go();
    hub.write(
        "c1",
        IoWrite {
            handle: opened.handle.clone(),
            data: Bytes(b"N".to_vec()),
        },
    );
    assert_eq!(data(&mut out).await, b"abc");
    assert_eq!(data(&mut out).await, b"");
}

#[tokio::test]
async fn reconnects_when_asked_and_refuses_when_not() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hub = IoHub::new();
    let (tx, mut out) = mpsc::unbounded_channel();
    hub.attach(tx);

    let options = IoOptions {
        reconnect: Some(true),
        backoff: Some(Backoff {
            initial_ms: Some(20),
            max_ms: Some(50),
        }),
        ..IoOptions::default()
    };
    let (opened, start) = hub.open("c1", tcp(port, options)).await.unwrap();
    start.go();
    // The peer hangs up; the core reconnects.
    let (socket, _) = listener.accept().await.unwrap();
    assert_eq!(
        state(&mut out).await.state,
        covalence_protocol::io::IoStateKind::Open
    );
    drop(socket);
    let lost = state(&mut out).await;
    assert_eq!(
        lost.state,
        covalence_protocol::io::IoStateKind::Reconnecting
    );
    let (mut socket, _) = tokio::time::timeout(WAIT, listener.accept())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state(&mut out).await.state,
        covalence_protocol::io::IoStateKind::Open
    );
    socket.write_all(b"back").await.unwrap();
    assert_eq!(data(&mut out).await, b"back");

    // Another connection can't use the handle.
    hub.write(
        "c2",
        IoWrite {
            handle: opened.handle.clone(),
            data: Bytes(b"no".to_vec()),
        },
    );
    hub.write(
        "c1",
        IoWrite {
            handle: opened.handle.clone(),
            data: Bytes(b"yes".to_vec()),
        },
    );
    let mut buf = [0u8; 8];
    let n = socket.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"yes");
    let traffic = hub.traffic("c1");
    assert_eq!(traffic.len(), 1);
    assert!(hub.traffic("c2").is_empty());

    // Without reconnect, a refused first attempt fails the open.
    drop(listener);
    let refused = hub.open("c1", tcp(port, IoOptions::default())).await;
    assert!(refused.is_err());

    // The host going closes sockets.
    hub.host_down();
    assert!(hub.traffic("c1").iter().all(|t| !t.open));
}
