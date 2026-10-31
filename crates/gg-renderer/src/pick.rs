//! Picks: questions of what the game's view shows at its pixels, which the frames that render it
//! read back from its IDs, and the frames two later answer.

use gg_gpu::MAX_READS;

/// The most picks a frame asks, in [`RenderData::picks`]: as many as it reads back.
///
/// [`RenderData::picks`]: crate::RenderData::picks
pub const MAX_PICKS: usize = MAX_READS;

/// A question of what the game's view shows at a pixel, which [`Renderer::picked`] answers with the
/// ID of the draw on top there, as [`Draws`] gives it, in the frame that reads it, once the GPU has
/// rendered that frame, as the frame two after it begins: the frame that asks it, but for those
/// [`Renderer::picked`] says. One outside the view is answered 0 by the frame that would read it.
///
/// [`Renderer::picked`]: crate::Renderer::picked
/// [`Draws`]: crate::Draws
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    /// What the answer comes back with, which the asker tells its picks apart by.
    pub tag: u64,
    /// The pixel's column and row in the view's image, from its top left.
    pub pixel: [u32; 2],
}

/// The picks a renderer has been asked, from asked to answered. Each step keeps its memory, so that
/// frames that pick no more than those before allocate nothing.
#[derive(Default)]
pub(crate) struct Picks {
    /// Those asked and not yet read, in the order they were asked.
    waiting: Vec<Pick>,
    /// The pixels the frame being rendered reads.
    pixels: Vec<[u32; 2]>,
    /// Those read and not yet answered, in the order they were read: the number of the frame that
    /// read each, with its tag.
    pending: Vec<(u64, u64)>,
    /// Those answered during the last render: each one's tag, with the ID read.
    answered: Vec<(u64, u64)>,
}

impl Picks {
    /// Starts a render that's asked `picks`: forgets the last render's answers, and has these wait
    /// after those waiting already, which a frame that didn't begin or read no picks left, and
    /// those past the [`MAX_READS`] a frame read after it. A frame asking at most [`MAX_PICKS`]
    /// keeps them few.
    ///
    /// # Panics
    ///
    /// If `picks` are more than [`MAX_PICKS`].
    pub(crate) fn ask(&mut self, picks: &[Pick]) {
        assert!(
            picks.len() <= MAX_PICKS,
            "a frame asks at most {MAX_PICKS} picks, as many as it reads; got {}",
            picks.len()
        );
        self.answered.clear();
        self.waiting.extend_from_slice(picks);
    }

    /// Answers those that frame `frame` read with `ids`, what it read, in order, as
    /// [`Frame::ids_read`](gg_gpu::Frame::ids_read) gives them.
    ///
    /// # Panics
    ///
    /// If `ids` aren't as many as frame `frame` read.
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
    /// the view, and returns their pixels, for it to read in that order: up to [`MAX_READS`] of
    /// them, in the order they were asked, the rest waiting for the frames after. Those outside the
    /// view as the frame renders it, every one if it renders none, are answered 0, as nothing of
    /// the view is there.
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

    /// Those answered during the last render: each one's tag, with the ID read.
    pub(crate) fn answered(&self) -> &[(u64, u64)] {
        &self.answered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Picks across frames as a renderer goes through them, each frame answering what the one two
    /// before it read: a pick outside the view answered 0 at once; IDs whose high halves are set;
    /// a frame that reads none, answered with none two later; more than a frame reads, the rest
    /// read by the next; picks while a frame doesn't begin, which wait for one that does; and a
    /// frame without the view, which answers 0 every pick waiting.
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

        // Frame 3 doesn't begin at first, so what it was asked waits with what it's asked next: more
        // than a frame reads, whose rest the next reads.
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
