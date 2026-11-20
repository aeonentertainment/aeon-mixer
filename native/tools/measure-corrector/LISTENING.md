# The blind listen

You are judging pitch correction on singers you do not know. Each source has three takes, A, B and
C: the same singing, processed three different ways. Nobody has told you which is which, and the
letters mean something different on every source. There are no wrong answers. What you hear is
what we need.

It takes about 25 minutes. Work alone, and do not compare notes with anyone until every sheet is
in.

## Set up

1. Use headphones you know, or good speakers in a quiet room. Not a laptop's speakers, not a phone's.
2. Open `answer-sheet.csv` in a spreadsheet and save a copy with your name on it.
3. Play `training/as-sung.wav`, then `training/hard-tuned.wav`. They are not part of the test. They
   are one singer, untouched and then corrected far too hard. Set your volume here and leave it for
   the rest of the listen. Listen for how the second one sounds: stepped, flat, or fluttering on the
   held notes.

## Listen

For each source folder, in any order:

1. Play A, B and C. Replay them as often as you like, in any order, and jump around inside them.
2. Rank the three takes by which you would rather hear the singer sound like: `1` is best. If you
   truly cannot prefer one over another, give them the same number. Equal is a fine answer.
3. Mark `warble` yes for any take where a held note wobbles, flutters or shimmers in a way that
   sounds like the machine and not the singer. A singer's own vibrato is not warble. If you cannot
   tell, write what you heard in `notes` and leave it blank.
4. Mark `robotic` yes for any take where held notes sound unnaturally still or stepped.
5. Write anything else in `notes`, with the time if you can: "B at 0:07, a click".

Take a break whenever your ears get tired. Do not try to work out which take is which.

## Send it back

Give the sheet to whoever gave you the folder. Fill one line per source, with your name in the
`listener` column on each.

## For whoever runs this

Keep `answer-key.json` to yourself until all three sheets are in. Then put them together, one header
and every listener's lines, and run:

```
cargo run --release -p measure-corrector -- score --sheet all-sheets.csv --key answer-key.json
```

The bar is the one in Aeon Karaoke's `docs/open-problems/auto-tune.md`: with at least three
listeners, every listener ranks melody above or equal to off on every source, and nobody reports
warble on a corrected take. When the key has a `before` take, the corrector a change replaces,
every listener must also rank melody above or equal to it. Warble reported on the off take fails nothing: it is the control, and
says how often that ear hears warble in the singer. Three listeners can show a problem; they cannot
show there is none, so quote the responses and not just the verdict.

The audio credits are in `ATTRIBUTION.md`.
