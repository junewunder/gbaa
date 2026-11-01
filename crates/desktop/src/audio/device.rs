//! The host's output device, through `cpal`: its callback drains the
//! [`Shared`] queue.

use super::{Shared, Stereo};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use std::sync::Arc;

/// A playing stream. Dropping it stops playback.
pub struct Output {
    _stream: cpal::Stream,
    pub name: String,
}

/// Open the default output device at its default rate and start it
/// playing from a new queue.
pub fn open() -> Result<(Output, Arc<Shared>), String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no default output device")?;
    let name = device
        .description()
        .map(|d| d.to_string())
        .unwrap_or_else(|_| "output device".into());
    let config = device
        .default_output_config()
        .map_err(|e| format!("{name}: {e}"))?;
    let format = config.sample_format();
    let config: StreamConfig = config.into();
    let shared = Shared::new(config.sample_rate);
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config, &shared),
        SampleFormat::I16 => build::<i16>(&device, config, &shared),
        SampleFormat::I32 => build::<i32>(&device, config, &shared),
        SampleFormat::U16 => build::<u16>(&device, config, &shared),
        f => return Err(format!("{name}: unsupported sample format {f}")),
    }
    .map_err(|e| format!("{name}: {e}"))?;
    stream.play().map_err(|e| format!("{name}: {e}"))?;
    Ok((
        Output {
            _stream: stream,
            name,
        },
        shared,
    ))
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: StreamConfig,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, cpal::Error> {
    let channels = config.channels as usize;
    let shared = shared.clone();
    // Room for the usual pulls, so the callback doesn't allocate.
    let mut frames: Vec<Stereo> = Vec::with_capacity(16384);
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            frames.resize(data.len() / channels, [0.0; 2]);
            shared.pull(&mut frames);
            interleave(&frames, channels, data);
        },
        |e| eprintln!("gbaa-desktop: audio: {e}"),
        None,
    )
}

/// Write stereo `frames` into a device buffer of `channels` channels: mono
/// gets the mix, and channels past the second get silence.
pub fn interleave<T: SizedSample + FromSample<f32>>(
    frames: &[Stereo],
    channels: usize,
    data: &mut [T],
) {
    for (out, &[l, r]) in data.chunks_mut(channels).zip(frames) {
        match out {
            [m] => *m = T::from_sample((l + r) / 2.0),
            [a, b, rest @ ..] => {
                *a = T::from_sample(l);
                *b = T::from_sample(r);
                rest.fill(T::EQUILIBRIUM);
            }
            [] => {}
        }
    }
}
