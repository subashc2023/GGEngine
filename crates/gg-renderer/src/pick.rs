//! Picks: what the game's view shows at given pixels, read back from its IDs by the frames that
//! render it and answered two frames later.

use gg_gpu::MAX_READS;

/// The most picks a frame asks, in [`RenderData::picks`]: as many as it reads back.
///
/// [`RenderData::picks`]: crate::RenderData::picks
pub const MAX_PICKS: usize = MAX_READS;

/// A question of what the game's view shows at a pixel.
///
/// A frame reads up to [`MAX_PICKS`] as it renders the view: first those left by frames with
/// nothing to render into or that read none, then its own. Later frames read the rest.
/// [`Renderer::picked`] answers each with the ID of the draw on top there ([`Draws`]), or 0 where
/// none drew, as the frame two after the reading one begins. One outside the view is answered 0 by
/// the frame that would read it.
///
/// [`Renderer::picked`]: crate::Renderer::picked
/// [`Draws`]: crate::Draws
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    /// Comes back with the answer, to tell picks apart by.
    pub tag: u64,
    /// Column and row in the view's image, from its top left.
    pub pixel: [u32; 2],
}

/// The picks a renderer has been asked, from asked to answered. Each step keeps its memory, so
/// frames that pick no more than those before allocate nothing.
#[derive(Default)]
pub(crate) struct Picks {
    /// Asked and not yet read, in order.
    waiting: Vec<Pick>,
    /// The pixels the frame being rendered reads.
    pixels: Vec<[u32; 2]>,
    /// Read and not yet answered, in order: the reading frame's number, and the tag.
    pending: Vec<(u64, u64)>,
    /// Answered during the last render: each tag, with the ID read.
    answered: Vec<(u64, u64)>,
}

impl Picks {
    /// Starts a render asked `picks`: forgets the last render's answers, and queues these after
    /// those already waiting, which a frame that didn't begin or read none left, or past the
    /// [`MAX_READS`] a frame read. A frame asking at most [`MAX_PICKS`] keeps them few.
    ///
    /// # Panics
    ///
    /// - If `picks` are more than [`MAX_PICKS`].
    pub(crate) fn ask(&mut self, picks: &[Pick]) {
        assert!(
            picks.len() <= MAX_PICKS,
            "a frame asks at most {MAX_PICKS} picks, as many as it reads; got {}",
            picks.len()
        );
        self.answered.clear();
        self.waiting.extend_from_slice(picks);
    }

    /// Answers those frame `frame` read with `ids`, in order, as
    /// [`Frame::ids_read`](gg_gpu::Frame::ids_read) gives them.
    ///
    /// # Panics
    ///
    /// - If `ids` aren't as many as frame `frame` read.
    pub(crate) fn answer(&mut self, (frame, ids): (u64, &[[u32; 2]])) {
        let read = (self.pending.iter())
            .take_while(|&&(by, _)| by == frame)
            .count();
        assert_eq!(
            read,
            ids.len(),
            "frame {frame} read back an ID for each pick it read"
        );
        let answers = (self.pending.drain(..read).zip(ids))
            .map(|((_, tag), &[low, high])| (tag, u64::from(low) | u64::from(high) << 32));
        self.answered.extend(answers);
    }

    /// Takes those waiting that frame `frame` reads of the view's IDs, `size` pixels, if it renders
    /// the view, and returns their pixels in reading order: up to [`MAX_READS`], in asking order,
    /// the rest waiting for later frames. Those outside the view as the frame renders it, or all if
    /// it renders none, are answered 0.
    pub(crate) fn take(&mut self, frame: u64, size: Option<[u32; 2]>) -> &[[u32; 2]] {
        self.pixels.clear();
        let mut kept = 0;
        for i in 0..self.waiting.len() {
            let pick = self.waiting[i];
            let [x, y] = pick.pixel;
            if !size.is_some_and(|[width, height]| x < width && y < height) {
                self.answered.push((pick.tag, 0));
            } else if self.pixels.len() < MAX_READS {
                self.pixels.push(pick.pixel);
                self.pending.push((frame, pick.tag));
            } else {
                self.waiting[kept] = pick;
                kept += 1;
            }
        }
        self.waiting.truncate(kept);
        &self.pixels
    }

    /// Answered during the last render: each tag, with the ID read.
    pub(crate) fn answered(&self) -> &[(u64, u64)] {
        &self.answered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Picks across frames, each frame answering what the one two before read: a pick outside the
    /// view answered 0 at once; IDs whose high halves are set; a frame reading none, answered with
    /// none two later; more than a frame reads, the next reading the rest; picks while a frame
    /// doesn't begin, waiting for one that does; and a frame without the view answering 0 every
    /// pick waiting.
    #[test]
    fn each_pick_is_answered_by_the_ids_its_frame_read() {
        let pick = |tag, pixel| Pick { tag, pixel };
        let mut picks = Picks::default();
        let in_view = Some([4, 3]);
        picks.ask(&[
            pick(1, [0, 0]),
            pick(2, [4, 0]),
            pick(3, [3, 2]),
            pick(4, [1, 3]),
        ]);
        picks.answer((0, &[]));
        assert_eq!(picks.take(1, in_view), [[0, 0], [3, 2]]);
        assert_eq!(picks.answered(), [(2, 0), (4, 0)]);

        // Frame 2 reads none.
        picks.ask(&[]);
        picks.answer((0, &[]));
        assert!(picks.take(2, in_view).is_empty());
        assert!(picks.answered().is_empty(), "the last render's");

        // Frame 3 doesn't begin at first, so what it was asked waits with what it's asked next:
        // more than a frame reads, the next reading the rest.
        let many: Vec<Pick> = (0..MAX_PICKS as u64 + 2)
            .map(|tag| pick(100 + tag, [tag as u32 % 4, 1]))
            .collect();
        let (first, rest) = many.split_at(MAX_PICKS);
        picks.ask(first);
        picks.ask(rest);
        picks.answer((1, &[[7, 0], [8, 1]]));
        assert_eq!(picks.answered(), [(1, 7), (3, 8 | 1 << 32)]);
        assert_eq!(picks.take(3, in_view).len(), MAX_READS);

        // Frame 4 doesn't begin at first, so its picks wait.
        picks.ask(&[pick(200, [2, 2])]);
        picks.ask(&[pick(201, [3, 0])]);
        picks.answer((2, &[]));
        let read = picks.take(4, in_view);
        assert_eq!(read, [[0, 1], [1, 1], [2, 2], [3, 0]]);
        assert!(picks.answered().is_empty());

        // Frame 5 shows no view.
        picks.ask(&[pick(300, [0, 0])]);
        let ids: Vec<[u32; 2]> = (0..MAX_READS as u32).map(|i| [i, 0]).collect();
        picks.answer((3, &ids));
        assert!(picks.take(5, None).is_empty());
        let answered = picks.answered();
        assert_eq!(answered.len(), MAX_READS + 1);
        assert_eq!(answered[MAX_READS - 1], (100 + MAX_READS as u64 - 1, 63));
        assert_eq!(answered[MAX_READS], (300, 0));

        picks.ask(&[]);
        picks.answer((4, &[[1, 0], [2, 0], [3, 0], [4, 0]]));
        let tags: Vec<(u64, u64)> = picks.answered().to_vec();
        let last = 100 + MAX_READS as u64;
        assert_eq!(tags, [(last, 1), (last + 1, 2), (200, 3), (201, 4)]);
    }

    #[test]
    #[should_panic(expected = "a frame asks at most 64 picks")]
    fn a_frame_asking_more_than_it_reads_is_a_bug() {
        let pick = Pick {
            tag: 0,
            pixel: [0, 0],
        };
        Picks::default().ask(&[pick; MAX_PICKS + 1]);
    }

    #[test]
    #[should_panic(expected = "read back an ID for each pick it read")]
    fn ids_for_other_picks_are_a_bug() {
        let mut picks = Picks::default();
        picks.ask(&[Pick {
            tag: 1,
            pixel: [0, 0],
        }]);
        picks.take(1, Some([1, 1]));
        picks.answer((1, &[]));
    }
}
