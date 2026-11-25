//! Fake backend for CI and development. It measures real wall-clock time per
//! segment and, when the segment ends, writes synthetic media of exactly that
//! length (test pattern + sine tone) with the ffmpeg CLI.

use std::time::Instant;

use lumos_model::manifest::TrackKind;
use lumos_model::sources::{CaptureSource, Device, Display, PermissionState, PermissionStatus, Rect};
use lumos_model::{ToolError, ToolResult};

use crate::{CaptureBackend, CaptureSession, SegmentOutput, SessionConfig, TrackInfo, segment_file};

#[derive(Default)]
pub struct FakeBackend;

const DISPLAY_ID: &str = "fake-1";

#[async_trait::async_trait]
impl CaptureBackend for FakeBackend {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn can_record(&self) -> bool {
        true
    }
    async fn permissions(&self) -> PermissionStatus {
        PermissionStatus::all(PermissionState::Granted)
    }
    async fn displays(&self) -> ToolResult<Vec<Display>> {
        Ok(vec![Display {
            id: DISPLAY_ID.into(),
            name: "Fake Display".into(),
            main: true,
            bounds: Rect { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 },
            scale: 1.0,
            refresh_hz: 60.0,
        }])
    }
    async fn cameras(&self) -> ToolResult<Vec<Device>> {
        Ok(vec![Device { id: "fake-cam".into(), name: "Fake Camera".into(), default: true }])
    }
    async fn microphones(&self) -> ToolResult<Vec<Device>> {
        Ok(vec![Device { id: "fake-mic".into(), name: "Fake Microphone".into(), default: true }])
    }
    async fn open(&self, config: SessionConfig) -> ToolResult<Box<dyn CaptureSession>> {
        match &config.source {
            CaptureSource::Display { display_id } if display_id == "main" || display_id == DISPLAY_ID => {}
            CaptureSource::Display { display_id } => return Err(ToolError::not_found(format!("no display '{display_id}'"))),
            other => return Err(ToolError::invalid_params(format!("the fake backend only records displays, not {other:?}"))),
        }
        let mut tracks = vec![TrackInfo { kind: TrackKind::Screen, codec: "h264".into(), device_name: None }];
        if config.camera.is_some() {
            tracks.push(TrackInfo { kind: TrackKind::Camera, codec: "h264".into(), device_name: Some("Fake Camera".into()) });
        }
        if config.mic.is_some() {
            tracks.push(TrackInfo { kind: TrackKind::Mic, codec: "aac".into(), device_name: Some("Fake Microphone".into()) });
        }
        if config.system_audio {
            tracks.push(TrackInfo { kind: TrackKind::SystemAudio, codec: "aac".into(), device_name: None });
        }
        for t in &tracks {
            std::fs::create_dir_all(config.bundle_dir.join("tracks").join(t.kind.dir()))
                .map_err(|e| ToolError::backend(format!("create track folder: {e}")))?;
        }
        Ok(Box::new(FakeSession { config, tracks, current: None }))
    }
}

struct FakeSession {
    config: SessionConfig,
    tracks: Vec<TrackInfo>,
    current: Option<(u32, Instant)>,
}

#[async_trait::async_trait]
impl CaptureSession for FakeSession {
    fn source_name(&self) -> String {
        "Fake Display".into()
    }
    fn tracks(&self) -> Vec<TrackInfo> {
        self.tracks.clone()
    }
    async fn begin_segment(&mut self, index: u32) -> ToolResult<()> {
        if self.current.is_some() {
            return Err(ToolError::invalid_state("segment already open"));
        }
        self.current = Some((index, Instant::now()));
        Ok(())
    }
    async fn end_segment(&mut self) -> ToolResult<SegmentOutput> {
        let (index, started) = self.current.take().ok_or_else(|| ToolError::invalid_state("no open segment"))?;
        let duration_us = started.elapsed().as_micros() as i64;
        let mut files = Vec::new();
        let mut bytes = 0;
        for t in &self.tracks {
            let rel = segment_file(t.kind, index);
            let path = self.config.bundle_dir.join(&rel);
            let res = match t.kind {
                TrackKind::Screen => lumos_media::generate_test_video(&path, duration_us, 1280, 720, self.config.fps).await,
                TrackKind::Camera => lumos_media::generate_test_video(&path, duration_us, 320, 320, self.config.fps).await,
                TrackKind::Mic => lumos_media::generate_test_tone(&path, duration_us, 1000, 1).await,
                TrackKind::SystemAudio => lumos_media::generate_test_tone(&path, duration_us, 440, 2).await,
            };
            res.map_err(|e| ToolError::backend(e.to_string()))?;
            bytes += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            files.push((t.kind, rel));
        }
        Ok(SegmentOutput { duration_us, files, bytes })
    }
    async fn close(self: Box<Self>) -> ToolResult<()> {
        Ok(())
    }
}
