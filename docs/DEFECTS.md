# Known defects

Defects left in because fixing them costs more than they do: each says what it costs, what the fix
would be, and when the fix would be worth it. [AGENTS.md](../AGENTS.md) says how to keep this file,
under Workflow; work put off until a phase or a need reaches it is in [PLAN.md](PLAN.md)'s Deferred.

## Performance

| Defect | Fix | Worth it when |
|---|---|---|
| While a file dialog is open, the main thread keeps about 80% of a core busy on Windows: 2.3 to 2.6 s of CPU over 2.7 to 3.2 s, against 0.9 s for the same dialog shown outside winit. `Context::pick_file` blocks the frame that asks, inside a winit callback, so winit, finding its handler taken, asks for a paint again at each `WM_PAINT` the dialog's modal loop passes it, and the loop never idles. Windows' window menu, which the UI's title bar opens, spins the same way. On Linux, where the portal's call blocks without passing the window its events, a compositor may say the app isn't responding (not checked) | Show the dialog on a thread of its own, as rfd's `AsyncFileDialog` does on Windows, with frames going on behind it and the path it picks reaching the layer a frame or more later; for the window menu, a window procedure in front of winit's that answers `WM_PAINT` while a callback runs, or a fix in winit | The spin shows, as on a laptop's battery or fan, or a Linux compositor flags the editor |

## Files

| Defect | Fix | Worth it when |
|---|---|---|
| A scene file, the dock's layout or the pipeline cache saved over a file with other hard links replaces that file rather than writing through it, so the other links keep what it held: writing whole renames a new file over it, which a link elsewhere doesn't follow | Write through a file with other links, in place, giving up writing it whole, or say so as it saves | Someone keeps scenes behind hard links |

## Editor

| Defect | Fix | Worth it when |
|---|---|---|
| The File menu's shortcuts, Ctrl+N, Ctrl+O, Ctrl+S and Ctrl+Shift+S, do nothing while a torn-off window has the keys: the editor reads them from the input of the main window, which its `ui` runs in, and a torn-off window's keys go to that window's own. The Inspector's Ctrl+Z works there, since it reads each window's input as its panels draw in it, and the File menu, in the main window, works from any | A `Ui` call that takes a shortcut's press from whichever window has the keys, as the Inspector's panels take theirs | Someone works in torn-off windows, or another layer's shortcuts need the same |
