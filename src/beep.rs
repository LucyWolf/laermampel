//! Kurzer Warnton über das Standard-Ausgabegerät.

use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

const FREQ_HZ: f32 = 880.0;
const LENGTH_SECONDS: f32 = 0.15;
const FADE_SECONDS: f32 = 0.01;

/// Spielt den Ton im Hintergrund ab. `volume` von 0.0 bis 1.0.
pub fn play(volume: f32) {
    let volume = volume.clamp(0.0, 1.0);
    std::thread::spawn(move || {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else { return };
        let Ok(config) = device.default_output_config() else { return };
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, volume),
            SampleFormat::I16 => build::<i16>(&device, &config, volume),
            SampleFormat::I32 => build::<i32>(&device, &config, volume),
            SampleFormat::U16 => build::<u16>(&device, &config, volume),
            _ => None,
        };
        let Some(stream) = stream else { return };
        if stream.play().is_ok() {
            std::thread::sleep(Duration::from_secs_f32(LENGTH_SECONDS + 0.1));
        }
    });
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    volume: f32,
) -> Option<cpal::Stream>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let sample_rate = config.sample_rate() as f32;
    let channels = config.channels().max(1) as usize;
    let total = (sample_rate * LENGTH_SECONDS) as usize;
    let fade = (sample_rate * FADE_SECONDS) as usize;
    let mut n = 0usize;

    device
        .build_output_stream(
            config.clone().into(),
            move |data: &mut [T], _: &_| {
                for frame in data.chunks_mut(channels) {
                    let value = if n < total {
                        // Weich ein- und ausblenden, sonst knackt es.
                        let envelope = (n.min(total - n) as f32 / fade as f32).min(1.0);
                        let t = n as f32 / sample_rate;
                        (2.0 * std::f32::consts::PI * FREQ_HZ * t).sin() * envelope * volume * 0.5
                    } else {
                        0.0
                    };
                    n += 1;
                    for s in frame {
                        *s = T::from_sample(value);
                    }
                }
            },
            |_| {},
            None,
        )
        .ok()
}
