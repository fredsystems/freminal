# Planning for freminal

## Purpose

Scratch notes so I don't forget things. I may specify a general timeline for when we should do them, but I may elect to promote up in the timeline.

### Items

#### Current Bugs

1. Right click paste does not work in the buffer.
2. We see excessively high "command duration time" in the gutter when the prompt is left alone, I type some command and then hit control-c. It takes the time from prompt showing up to the control-c being done, when no command was actually executed.
3. Related to #2, we also get a toast/system notification when you control-c a command that never was executed.
4. Gutter coloring may extend past the current command prompt and go to the bottom of the pane. It should always stop at the command prompt, OR the last line of output text of a running command.
5. "2026-06-11T16:13:06.465926Z WARN egui_glow::painter:753: You forgot to call destroy() on the egui glow painter. Resources will leak!" is a warning emitted at app close. Realistically, leaking at close is probably not a big deal, but we should tear down properly.
6. We have a pattern on closing a pane/tab that feels like we're sequecing the close wrong.
7. Builds via cargo, on the about modal, show `Build unknow`
8. Builds via CI, ie the distributed versions, show `  freminal 0.9.0-beta.3 (unknown)` on tools like fastfetch. This is strongly related to #7 above, I'm sure. We should be embedding a build time, or commit hash, or something, I would think?
9. Command History pallete sort order is wrong. It has the oldest command listed first, when it should sort by most recent.

```bash
2026-06-11T16:13:56.518745Z ERROR freminal_terminal_emulator::io::pty:583: Failed to send data to terminal: sending on a disconnected channel
2026-06-11T16:13:56.518757Z ERROR freminal_terminal_emulator::io::pty:583: Failed to send data to terminal: sending on a disconnected channel
2026-06-11T16:13:56.519584Z  INFO freminal_terminal_emulator::io::pty:538: Child process exited with status: ExitStatus { code: 1, signal: None }
2026-06-11T16:13:56.519592Z  INFO freminal_terminal_emulator::io::pty:538: Child process exited with status: ExitStatus { code: 1, signal: None }
```

#### Pre-1.0 Remediations and Improvements

##### UI asthetic is very bare bones

We need a major beautification pass. All of the UI modals, and toast notifications, need styling and theming with the user selected theme. The buffer should NOT be affected. Things to research: is this platform specific, are we hamstrung by the underlying window toolkit we've built on (egui, like that). We should take inspiration from ACARS Hub, and in general web apps with rounded corners, consistent colour theming, etc.

Specific issues:

- Colour does not match the theme
- Many places we are using an emoji/powerline glyph of some kind for actions (close, recording, broadcast keyboard, etc). Some we've replaced so we get at least some kind of symbol, but others we haven't. I see squares on Linux in some instances. Not really tested other Linux distros to see if it's an emoji font issue, or other OSs like macOS or Windows. Realistically if we're relying on the font (or any fallible resource that may or may not have what we want) to supply us with the necessary glyphs, we've failed. We should provide the asset so we always know what it looks like and matches our aesthetic. My thinking is we bundle in svgs of some kind for all of these assets, but I'm happy to discuss alternatives.

##### Built in fonts

Whatever option is gone with, we should strive to ensure maximum ligature AND powerline glyph support is built in out of the box. I would like to investigate doing one, or more, of the following:

- Change the bundled font to something like a Nerd Font. I believe the license for nerd fonts are generally conducive to doing so, but I want to be sure. I believe this follows the model other terminals (at least, WezTerm?) use.
- Detect the platform default monospace font and use THAT over a built in font. I don't necessarily favour this option because the platform font likely does not include ligatures, powerline glyphs, etc.

##### Focus

Keyboard (more accurately, active pane) follows mouse. I routinely get annoyed that I mouse to a pane in a muxxed session and then I have to click to focus. Default should be pane focus (not tab, tab switching should be click focus) follows mouse. User toggleable to the current model of click to focus.

##### About freminal

We have vendored a few things, or included resources with licenses that work with our MIT, but may require direct attribution. I want to ensure we're complying with those requirements (also, giving credit where credit is due is the correct thing to do), so I would like a specific document calling out specific attributions (portable-pty, and the bundled fonts come to mind. Not exhaustive), AND I would like the README.md to reflect that (maybe just link to the doc?), AND the about modal should have attibution/link back to the doc, too.

##### Muxxing visuals

Currently, the visuals for muxxing are slightly wrong. The "highlight" is a bit misleading. Look at the following layout

```text
|----------------|
| Pane 1         |
|----------------|
| Pane 2 | Pane 3|
|----------------|
```

If Pane 1 is focused, I would expect the divider bar between the bottom and top panes to be fully blue (or whatever the color is....are we themeing it? Should audit). I would expect the half coloring if we've focused a pane in the bottom _on the side of the pane that is focused_, as well as _the entire divider bar between the two side by side panes_. Currently, it chooses to fill in the horizontal divider half on the left side if P1 is focused, with P2 focused it goes to the right, and the top half of the vert bar. P3 focused it chooses the right and bottom half of the vert bar.

Basically, in as few words as I can put it: the active pane highlighting should _surround_ the active pane, excluding the top, bottom, left and right edges of the entire buffer window itself.
