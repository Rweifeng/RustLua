//! Minimal sound-effect playback backed by rodio: WAV clips loaded from the
//! project assets, a master volume, and a lazily-opened output device so games
//! run fine on machines without audio hardware (play degrades to a warning).

use std::{collections::HashMap, error::Error, fmt, io::Cursor, path::PathBuf};

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

/// Errors produced while configuring or playing audio.
#[derive(Debug)]
pub enum AudioError {
    Io(String),
    Device(String),
    Decode(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(reason) => write!(f, "audio I/O failed: {reason}"),
            Self::Device(reason) => write!(f, "audio device failed: {reason}"),
            Self::Decode(reason) => write!(f, "audio decode failed: {reason}"),
        }
    }
}

impl Error for AudioError {}

/// Plays named WAV effects from an asset directory. The output device is
/// opened on first play and kept alive; finished sinks are reaped so the
/// underlying mixer slots are recycled.
pub struct AudioSystem {
    asset_dir: Option<PathBuf>,
    sounds: HashMap<String, Vec<u8>>,
    volume: f32,
    stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    sinks: Vec<Sink>,
}

impl fmt::Debug for AudioSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioSystem")
            .field("cached_sounds", &self.sounds.len())
            .field("active_sinks", &self.sinks.len())
            .field("volume", &self.volume)
            .field("device_open", &self.stream.is_some())
            .finish()
    }
}

impl Default for AudioSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSystem {
    /// Creates an audio system with no device open and no asset directory.
    #[must_use]
    pub fn new() -> Self {
        Self {
            asset_dir: None,
            sounds: HashMap::new(),
            volume: 1.0,
            stream: None,
            handle: None,
            sinks: Vec::new(),
        }
    }

    /// Sets the directory WAV clips are loaded from (usually `<project>/assets`).
    pub fn set_asset_dir(&mut self, dir: PathBuf) {
        self.asset_dir = Some(dir);
    }

    /// Sets the master volume in `0..=1`.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// Returns the master volume.
    #[must_use]
    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// Plays `name.wav` from the asset directory. Missing clips or a missing
    /// audio device are logged and degrade gracefully instead of failing.
    pub fn play(&mut self, name: &str) -> Result<(), AudioError> {
        let bytes = self.sound_bytes(name)?;
        if self.stream.is_none() {
            match OutputStream::try_default() {
                Ok((stream, handle)) => {
                    self.stream = Some(stream);
                    self.handle = Some(handle);
                }
                Err(error) => {
                    tracing::warn!(target: "audio", error = %error, "[audio] no output device; sound effects disabled");
                    return Ok(());
                }
            }
        }
        let handle = self
            .handle
            .as_ref()
            .ok_or_else(|| AudioError::Device("audio device handle unavailable".to_owned()))?;
        let sink = Sink::try_new(handle).map_err(|error| AudioError::Device(error.to_string()))?;
        let source = Decoder::new(Cursor::new(bytes))
            .map_err(|error| AudioError::Decode(error.to_string()))?;
        sink.set_volume(self.volume);
        sink.append(source);
        self.sinks.push(sink);
        self.sinks.retain(|sink| !sink.empty());
        tracing::info!(target: "audio", name, "[audio] played");
        Ok(())
    }

    fn sound_bytes(&mut self, name: &str) -> Result<Vec<u8>, AudioError> {
        if let Some(bytes) = self.sounds.get(name) {
            return Ok(bytes.clone());
        }
        let Some(dir) = &self.asset_dir else {
            return Err(AudioError::Io("no asset directory configured".to_owned()));
        };
        // Try `.wav` first, then `.ogg` (rodio decodes both from bytes).
        let mut loaded = None;
        for file in [format!("{name}.wav"), format!("{name}.ogg")] {
            let path = dir.join(&file);
            if path.is_file() {
                let bytes = std::fs::read(&path)
                    .map_err(|error| AudioError::Io(format!("{}: {error}", path.display())))?;
                loaded = Some(bytes);
                break;
            }
        }
        let bytes =
            loaded.ok_or_else(|| AudioError::Io(format!("no .wav/.ogg clip found for {name}")))?;
        self.sounds.insert(name.to_owned(), bytes.clone());
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_is_clamped_and_reported() {
        let mut audio = AudioSystem::new();
        audio.set_volume(1.7);
        assert_eq!(audio.volume(), 1.0);
        audio.set_volume(-0.2);
        assert_eq!(audio.volume(), 0.0);
    }

    #[test]
    fn missing_clip_returns_io_error_without_opening_a_device() {
        let mut audio = AudioSystem::new();
        audio.set_asset_dir(std::env::temp_dir());
        // loading fails before any device is opened
        assert!(audio.play("p2-audio-definitely-missing").is_err());
        assert!(
            audio.stream.is_none(),
            "device must not open on a failed load"
        );
    }
}
