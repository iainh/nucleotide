# Integrated terminal

Terminal tabs retain their output after the command exits. You can still scroll
through history and resize the terminal until you close the tab.

Selections follow their text during scrolling. They clear when switching screen
buffers, resizing the grid, resetting the terminal or discarding old history.
This avoids highlighting unrelated text after coordinates change.

## Clipboard

Select text with the pointer, then use Copy from the application menu or
Ctrl+Shift+C. Paste uses the application menu, Ctrl+Shift+V or Shift+Insert.
On macOS, Command+C and Command+V also work. Ctrl+C remains a terminal interrupt.

Paste uses the application's negotiated bracketed-paste mode. Without that mode,
newlines become carriage returns, as if entered at the terminal. Control bytes
such as Escape are replaced with spaces to prevent pasted escape sequences from
ending bracketed paste early. Pasting into an exited terminal does nothing.

## Mouse-enabled applications

When a terminal application enables mouse tracking, clicks, drags and wheel
events go to that application using its negotiated protocol. Hold Shift to
select text or scroll terminal history locally instead. A drag keeps the owner
it had at mouse-down, even if you press or release Shift before mouse-up.
