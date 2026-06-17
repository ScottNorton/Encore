//! Wyoming protocol framing.
//!
//! Wire format per event:
//! ```text
//! {"type":"...","version":"1.0.0","data_length":N,"payload_length":M}\n
//! [N bytes of JSON data]
//! [M bytes of binary payload]
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// A Wyoming protocol event.
#[derive(Debug, Clone)]
pub struct WyomingEvent {
    pub event_type: String,
    pub data: Value,
    pub payload: Vec<u8>,
}

/// Wire header.
#[derive(Serialize, Deserialize)]
struct Header {
    #[serde(rename = "type")]
    event_type: String,
    version: String,
    data_length: usize,
    payload_length: usize,
}

impl WyomingEvent {
    pub fn new(event_type: &str, data: Value) -> Self {
        Self {
            event_type: event_type.to_string(),
            data,
            payload: Vec::new(),
        }
    }

    pub fn with_payload(event_type: &str, data: Value, payload: Vec<u8>) -> Self {
        Self {
            event_type: event_type.to_string(),
            data,
            payload,
        }
    }
}

/// Read a single event from the TCP stream.
pub async fn read_event(reader: &mut BufReader<OwnedReadHalf>) -> Result<Option<WyomingEvent>> {
    // Read header line
    let mut header_line = String::new();
    let n = reader
        .read_line(&mut header_line)
        .await
        .context("read header")?;

    if n == 0 {
        return Ok(None); // connection closed
    }

    let header: Header = serde_json::from_str(header_line.trim()).context("parse header JSON")?;

    // Read data JSON
    let data = if header.data_length > 0 {
        let mut data_buf = vec![0u8; header.data_length];
        reader
            .read_exact(&mut data_buf)
            .await
            .context("read data")?;
        serde_json::from_slice(&data_buf).context("parse data JSON")?
    } else {
        Value::Object(Default::default())
    };

    // Read binary payload
    let payload = if header.payload_length > 0 {
        let mut payload_buf = vec![0u8; header.payload_length];
        reader
            .read_exact(&mut payload_buf)
            .await
            .context("read payload")?;
        payload_buf
    } else {
        Vec::new()
    };

    Ok(Some(WyomingEvent {
        event_type: header.event_type,
        data,
        payload,
    }))
}

/// Serialize an event to bytes (header line + data + payload).
/// Useful for testing and for pre-serializing events.
pub fn serialize_event(event: &WyomingEvent) -> Result<Vec<u8>> {
    let data_bytes = serde_json::to_vec(&event.data).context("serialize data")?;

    let header = Header {
        event_type: event.event_type.clone(),
        version: "1.0.0".to_string(),
        data_length: data_bytes.len(),
        payload_length: event.payload.len(),
    };

    let header_line = serde_json::to_string(&header).context("serialize header")?;

    let mut out = Vec::new();
    out.extend_from_slice(header_line.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(&data_bytes);
    out.extend_from_slice(&event.payload);
    Ok(out)
}

/// Write a single event to the TCP stream.
pub async fn write_event(writer: &mut OwnedWriteHalf, event: &WyomingEvent) -> Result<()> {
    let data_bytes = serde_json::to_vec(&event.data).context("serialize data")?;

    let header = Header {
        event_type: event.event_type.clone(),
        version: "1.0.0".to_string(),
        data_length: data_bytes.len(),
        payload_length: event.payload.len(),
    };

    let header_line = serde_json::to_string(&header).context("serialize header")?;

    writer
        .write_all(header_line.as_bytes())
        .await
        .context("write header")?;
    writer.write_all(b"\n").await.context("write newline")?;

    if !data_bytes.is_empty() {
        writer.write_all(&data_bytes).await.context("write data")?;
    }

    if !event.payload.is_empty() {
        writer
            .write_all(&event.payload)
            .await
            .context("write payload")?;
    }

    writer.flush().await.context("flush")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_new_has_empty_payload() {
        let event = WyomingEvent::new("describe", json!({"version": "1.0"}));
        assert_eq!(event.event_type, "describe");
        assert!(event.payload.is_empty());
        assert_eq!(event.data["version"], "1.0");
    }

    #[test]
    fn event_with_payload() {
        let audio = vec![0u8; 1024];
        let event = WyomingEvent::with_payload(
            "audio-chunk",
            json!({"rate": 16000, "width": 2, "channels": 1}),
            audio.clone(),
        );
        assert_eq!(event.event_type, "audio-chunk");
        assert_eq!(event.payload.len(), 1024);
        assert_eq!(event.data["rate"], 16000);
    }

    #[test]
    fn serialize_event_produces_valid_wire_format() {
        let event = WyomingEvent::new("info", json!({"name": "Invoke"}));
        let bytes = serialize_event(&event).unwrap();
        let wire = String::from_utf8_lossy(&bytes);

        // Should contain header line ending with \n
        let newline_pos = wire.find('\n').expect("header should end with newline");
        let header_str = &wire[..newline_pos];

        // Header should be valid JSON with expected fields
        let header: serde_json::Value = serde_json::from_str(header_str).unwrap();
        assert_eq!(header["type"], "info");
        assert_eq!(header["version"], "1.0.0");
        assert!(header["data_length"].as_u64().unwrap() > 0);
        assert_eq!(header["payload_length"], 0);

        // Data portion should be valid JSON
        let data_len = header["data_length"].as_u64().unwrap() as usize;
        let data_start = newline_pos + 1;
        let data_str = &wire[data_start..data_start + data_len];
        let data: serde_json::Value = serde_json::from_str(data_str).unwrap();
        assert_eq!(data["name"], "Invoke");
    }

    #[test]
    fn serialize_event_with_payload_includes_binary() {
        let payload = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let event =
            WyomingEvent::with_payload("audio-chunk", json!({"rate": 16000}), payload.clone());
        let bytes = serialize_event(&event).unwrap();

        // Last 4 bytes should be our payload
        assert_eq!(&bytes[bytes.len() - 4..], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn serialize_event_empty_data() {
        let event = WyomingEvent::new("pong", json!({}));
        let bytes = serialize_event(&event).unwrap();
        let wire = String::from_utf8_lossy(&bytes);

        let newline_pos = wire.find('\n').unwrap();
        let header: serde_json::Value = serde_json::from_str(&wire[..newline_pos]).unwrap();
        assert_eq!(header["type"], "pong");
        // Even empty object {} has a non-zero data_length (2 bytes)
        assert!(header["data_length"].as_u64().unwrap() >= 2);
    }

    /// Full round-trip test: serialize event, feed to reader, get back same event.
    #[tokio::test]
    async fn write_then_read_round_trip() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let original = WyomingEvent::with_payload(
            "audio-chunk",
            json!({"rate": 16000, "width": 2, "channels": 1}),
            vec![1, 2, 3, 4, 5, 6],
        );

        // Writer side
        let orig_clone = original.clone();
        let writer_task = tokio::spawn(async move {
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let (_, write_half) = stream.into_split();
            let mut writer = write_half;
            write_event(&mut writer, &orig_clone).await.unwrap();
        });

        // Reader side
        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, _) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        let received = read_event(&mut reader).await.unwrap().unwrap();

        assert_eq!(received.event_type, "audio-chunk");
        assert_eq!(received.data["rate"], 16000);
        assert_eq!(received.data["width"], 2);
        assert_eq!(received.data["channels"], 1);
        assert_eq!(received.payload, vec![1, 2, 3, 4, 5, 6]);

        writer_task.await.unwrap();
    }

    /// Multiple events on same connection.
    #[tokio::test]
    async fn multiple_events_on_same_stream() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let writer_task = tokio::spawn(async move {
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let (_, write_half) = stream.into_split();
            let mut writer = write_half;

            let events = vec![
                WyomingEvent::new("describe", json!({})),
                WyomingEvent::new("info", json!({"name": "test"})),
                WyomingEvent::new("run-satellite", json!({})),
            ];
            for ev in &events {
                write_event(&mut writer, ev).await.unwrap();
            }
        });

        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, _) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        let ev1 = read_event(&mut reader).await.unwrap().unwrap();
        assert_eq!(ev1.event_type, "describe");

        let ev2 = read_event(&mut reader).await.unwrap().unwrap();
        assert_eq!(ev2.event_type, "info");
        assert_eq!(ev2.data["name"], "test");

        let ev3 = read_event(&mut reader).await.unwrap().unwrap();
        assert_eq!(ev3.event_type, "run-satellite");

        writer_task.await.unwrap();
    }

    /// Connection close returns None.
    #[tokio::test]
    async fn read_event_returns_none_on_close() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let writer_task = tokio::spawn(async move {
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            drop(stream); // close immediately
        });

        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, _) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        writer_task.await.unwrap();

        let result = read_event(&mut reader).await.unwrap();
        assert!(result.is_none());
    }
}
