//! The pixels of one tab, as Chromium painted them off-screen.
//!
//! CEF paints the page (`PET_VIEW`) and, while a `<select>` or similar
//! widget is open, a popup (`PET_POPUP`) that belongs over the page at a
//! given rectangle. Both arrive as BGRA buffers in physical pixels; the
//! frame keeps the latest of each and composes them when the window asks
//! for pixels.

use std::borrow::Cow;

/// A BGRA image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Image {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

impl Image {
    fn set(&mut self, width: u32, height: u32, bgra: &[u8]) {
        self.width = width;
        self.height = height;
        self.bgra.clear();
        self.bgra.extend_from_slice(bgra);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    view: Image,
    popup: Image,
    /// Where the popup goes, in physical pixels relative to the view.
    popup_rect: (i32, i32),
    popup_shown: bool,
    dirty: bool,
}

impl Frame {
    /// Width and height in physical pixels; zero before the first paint.
    pub fn size(&self) -> (u32, u32) {
        (self.view.width, self.view.height)
    }

    pub fn is_empty(&self) -> bool {
        self.view.bgra.is_empty()
    }

    /// Whether anything was painted since the last call, which clears it.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The view with the popup composed over it, BGRA, row-major, tightly
    /// packed (`size()` pixels).
    pub fn pixels(&self) -> Cow<'_, [u8]> {
        if !self.popup_shown || self.popup.bgra.is_empty() || self.view.bgra.is_empty() {
            return Cow::Borrowed(&self.view.bgra);
        }
        let mut out = self.view.bgra.clone();
        let (vw, vh) = (i64::from(self.view.width), i64::from(self.view.height));
        let (px, py) = (i64::from(self.popup_rect.0), i64::from(self.popup_rect.1));
        for row in 0..i64::from(self.popup.height) {
            let y = py + row;
            if y < 0 || y >= vh {
                continue;
            }
            for col in 0..i64::from(self.popup.width) {
                let x = px + col;
                if x < 0 || x >= vw {
                    continue;
                }
                let src = ((row * i64::from(self.popup.width) + col) * 4) as usize;
                let dst = ((y * vw + x) * 4) as usize;
                out[dst..dst + 4].copy_from_slice(&self.popup.bgra[src..src + 4]);
            }
        }
        Cow::Owned(out)
    }

    pub(crate) fn paint_view(&mut self, width: u32, height: u32, bgra: &[u8]) {
        self.view.set(width, height, bgra);
        self.dirty = true;
    }

    pub(crate) fn paint_popup(&mut self, width: u32, height: u32, bgra: &[u8]) {
        self.popup.set(width, height, bgra);
        self.dirty = true;
    }

    pub(crate) fn set_popup_rect(&mut self, x: i32, y: i32) {
        self.popup_rect = (x, y);
        self.dirty = true;
    }

    pub(crate) fn show_popup(&mut self, shown: bool) {
        self.popup_shown = shown;
        if !shown {
            self.popup = Image::default();
        }
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::Frame;

    fn pixel(frame: &Frame, x: usize, y: usize) -> [u8; 4] {
        let pixels = frame.pixels();
        let i = (y * frame.size().0 as usize + x) * 4;
        pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn a_painted_view_is_dirty_once() {
        let mut frame = Frame::default();
        assert!(frame.is_empty());
        frame.paint_view(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(frame.size(), (2, 1));
        assert!(frame.take_dirty());
        assert!(!frame.take_dirty());
        assert_eq!(pixel(&frame, 1, 0), [5, 6, 7, 8]);
    }

    #[test]
    fn a_shown_popup_is_composed_over_the_view_and_clipped() {
        let mut frame = Frame::default();
        frame.paint_view(3, 3, &[0; 36]);
        frame.set_popup_rect(2, 2);
        frame.paint_popup(2, 2, &[9; 16]);
        // Hidden: the view alone.
        assert_eq!(pixel(&frame, 2, 2), [0; 4]);
        frame.show_popup(true);
        assert_eq!(pixel(&frame, 2, 2), [9; 4]);
        assert_eq!(pixel(&frame, 1, 1), [0; 4]);
        assert_eq!(frame.pixels().len(), 36);
        frame.show_popup(false);
        assert_eq!(pixel(&frame, 2, 2), [0; 4]);
    }
}
