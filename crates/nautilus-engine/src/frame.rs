//! The pixels of one tab, as Chromium painted them off-screen.
//!
//! CEF paints the page (`PET_VIEW`) and, while a `<select>` or similar
//! widget is open, a popup (`PET_POPUP`) that belongs over the page at a
//! given rectangle. Both arrive as BGRA buffers in physical pixels; the
//! frame keeps the latest of each and composes them when the window asks
//! for pixels.
//!
//! CEF names the rectangles a view paint changed. When every change
//! since the pixels were last handed out was a view paint of the same
//! size, the frame copies only those rectangles in and reports them, so
//! the shell uploads only those; anything else (a size change, the
//! popup) makes the whole frame dirty.

use std::borrow::Cow;

use osmena_engine::PixelRect;

/// More dirty rectangles than this and the whole frame is reported
/// instead: one upload beats many small ones.
const MAX_DIRTY_RECTS: usize = 32;

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

    /// Copy `rects` of `bgra`, an image of this one's size, over the
    /// same places here; the rest stays. Rectangles are clipped.
    fn copy_rects(&mut self, bgra: &[u8], rects: &[PixelRect]) {
        let row_bytes = self.width as usize * 4;
        for rect in rects {
            let Some(rect) = clip(*rect, self.width, self.height) else {
                continue;
            };
            let bytes = rect.width as usize * 4;
            for row in rect.y..rect.y + rect.height {
                let start = row as usize * row_bytes + rect.x as usize * 4;
                self.bgra[start..start + bytes].copy_from_slice(&bgra[start..start + bytes]);
            }
        }
    }
}

/// `rect` clipped to `width` by `height`; `None` if nothing is left.
fn clip(rect: PixelRect, width: u32, height: u32) -> Option<PixelRect> {
    let right = rect.x.checked_add(rect.width)?.min(width);
    let bottom = rect.y.checked_add(rect.height)?.min(height);
    (rect.x < right && rect.y < bottom).then_some(PixelRect {
        x: rect.x,
        y: rect.y,
        width: right - rect.x,
        height: bottom - rect.y,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    view: Image,
    popup: Image,
    /// Where the popup goes, in physical pixels relative to the view.
    popup_rect: (i32, i32),
    popup_shown: bool,
    dirty: bool,
    /// The parts of the view changed since the last `take_dirty`, while
    /// `partial` says they are the whole story.
    dirty_rects: Vec<PixelRect>,
    /// False once something changed since the last `take_dirty` that
    /// `dirty_rects` cannot name: a paint of another size, the popup.
    partial: bool,
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
    /// [`Frame::dirty_rects`] keeps naming what changed until the next
    /// paint.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The parts of the pixels that changed since the hand-out before
    /// the last `take_dirty`, when they are known; empty means all of
    /// them.
    pub fn dirty_rects(&self) -> &[PixelRect] {
        if self.partial { &self.dirty_rects } else { &[] }
    }

    /// Something is about to change. The first change after a hand-out
    /// starts a new account of what changed.
    fn begin_change(&mut self) {
        if !self.dirty {
            self.dirty = true;
            self.dirty_rects.clear();
            self.partial = true;
        }
    }

    /// Something changed that no rectangle names.
    fn change_all(&mut self) {
        self.begin_change();
        self.partial = false;
        self.dirty_rects.clear();
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

    /// The view was painted: all of `bgra`, or, at an unchanged size,
    /// only `rects` of it (empty means all of it).
    pub(crate) fn paint_view(&mut self, width: u32, height: u32, bgra: &[u8], rects: &[PixelRect]) {
        let same_size = (width, height) == (self.view.width, self.view.height)
            && self.view.bgra.len() == bgra.len();
        if same_size && !rects.is_empty() {
            self.begin_change();
            self.view.copy_rects(bgra, rects);
            if self.partial {
                self.dirty_rects.extend_from_slice(rects);
                if self.dirty_rects.len() > MAX_DIRTY_RECTS {
                    self.change_all();
                }
            }
        } else {
            self.change_all();
            self.view.set(width, height, bgra);
        }
    }

    pub(crate) fn paint_popup(&mut self, width: u32, height: u32, bgra: &[u8]) {
        self.change_all();
        self.popup.set(width, height, bgra);
    }

    pub(crate) fn set_popup_rect(&mut self, x: i32, y: i32) {
        self.change_all();
        self.popup_rect = (x, y);
    }

    pub(crate) fn show_popup(&mut self, shown: bool) {
        self.change_all();
        self.popup_shown = shown;
        if !shown {
            self.popup = Image::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Frame, PixelRect};

    fn pixel(frame: &Frame, x: usize, y: usize) -> [u8; 4] {
        let pixels = frame.pixels();
        let i = (y * frame.size().0 as usize + x) * 4;
        pixels[i..i + 4].try_into().unwrap()
    }

    fn rect(x: u32, y: u32, width: u32, height: u32) -> PixelRect {
        PixelRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_painted_view_is_dirty_once() {
        let mut frame = Frame::default();
        assert!(frame.is_empty());
        frame.paint_view(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8], &[]);
        assert_eq!(frame.size(), (2, 1));
        assert!(frame.take_dirty());
        assert!(!frame.take_dirty());
        assert_eq!(pixel(&frame, 1, 0), [5, 6, 7, 8]);
        assert!(frame.dirty_rects().is_empty(), "a first paint is all of it");
    }

    /// A same-size paint with rectangles copies only those in and names
    /// them until the next paint after a hand-out; a paint of another
    /// size, or any popup change, makes the whole frame dirty again.
    #[test]
    fn same_size_paints_change_only_their_rectangles() {
        let mut frame = Frame::default();
        frame.paint_view(3, 3, &[0; 36], &[]);
        frame.take_dirty();

        // A buffer of nines, but only (1,1) and (2,0) are claimed.
        frame.paint_view(3, 3, &[9; 36], &[rect(1, 1, 1, 1)]);
        frame.paint_view(3, 3, &[9; 36], &[rect(2, 0, 1, 1)]);
        assert_eq!(pixel(&frame, 1, 1), [9; 4]);
        assert_eq!(pixel(&frame, 2, 0), [9; 4]);
        assert_eq!(
            pixel(&frame, 0, 0),
            [0; 4],
            "unclaimed pixels keep their value"
        );
        assert!(frame.take_dirty());
        assert_eq!(frame.dirty_rects(), [rect(1, 1, 1, 1), rect(2, 0, 1, 1)]);
        // Still named after the hand-out, until the next change.
        assert!(!frame.take_dirty());
        assert_eq!(frame.dirty_rects().len(), 2);

        // The next change starts a new account.
        frame.paint_view(3, 3, &[7; 36], &[rect(0, 0, 1, 1)]);
        assert_eq!(frame.dirty_rects(), [rect(0, 0, 1, 1)]);
        // A rectangle off the edge is clipped, not a panic.
        frame.paint_view(3, 3, &[7; 36], &[rect(2, 2, 5, 5)]);
        assert_eq!(pixel(&frame, 2, 2), [7; 4]);
        assert!(frame.take_dirty());

        // A popup change: everything.
        frame.paint_view(3, 3, &[5; 36], &[rect(0, 0, 1, 1)]);
        frame.show_popup(true);
        assert!(frame.dirty_rects().is_empty());
        frame.take_dirty();
        // Another size: everything, and the pixels are replaced.
        frame.paint_view(2, 2, &[1; 16], &[rect(0, 0, 1, 1)]);
        assert_eq!(frame.size(), (2, 2));
        assert!(frame.dirty_rects().is_empty());
    }

    #[test]
    fn a_shown_popup_is_composed_over_the_view_and_clipped() {
        let mut frame = Frame::default();
        frame.paint_view(3, 3, &[0; 36], &[]);
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
