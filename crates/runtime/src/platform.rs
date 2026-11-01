//! What a front end provides: somewhere to show a frame, the keys, and
//! somewhere to play sound. The desktop backend (#29) implements
//! this first; an ESP32 backend may follow (decision 13).

pub use crate::sound::AudioChunk;

/// Screen width in pixels.
pub const WIDTH: usize = 240;
/// Screen height in pixels.
pub const HEIGHT: usize = 160;

/// One frame, row by row, each pixel in the GBA's own 15-bit BGR format
/// (`0bbbbbgggggrrrrr`). The backend converts it to whatever it displays.
pub type Frame = [u16; WIDTH * HEIGHT];

/// A front end.
pub trait Platform {
    /// Show a finished frame. Called once a frame, at the start of V-blank.
    fn present(&mut self, frame: &Frame);

    /// The keys held down now, as [`crate::io::key`] bits (1 = pressed).
    /// Polled once a frame.
    fn keys(&mut self) -> u16;

    /// The sound since the last call: the FIFOs' raw samples and the same
    /// mapped to stereo ([`AudioChunk`]). Called once a frame, before
    /// [`Platform::present`], and in between when the buffer is half
    /// full. The default drops it.
    fn audio(&mut self, _chunk: &AudioChunk<'_>) {}
}

/// A platform with no screen, no keys and no sound, for tests and headless
/// runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Headless;

impl Platform for Headless {
    fn present(&mut self, _frame: &Frame) {}

    fn keys(&mut self) -> u16 {
        0
    }
}
