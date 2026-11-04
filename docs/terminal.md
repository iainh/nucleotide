# Integrated terminal

Terminal tabs retain their output after the command exits. You can still scroll
through history and resize the terminal until you close the tab.

## Clipboard

Select text with the pointer, then use Copy from the application menu or
Ctrl+Shift+C. Paste uses the application menu, Ctrl+Shift+V or Shift+Insert.
On macOS, Command+C and Command+V also work. Ctrl+C remains a terminal interrupt.

Paste uses the application's negotiated bracketed-paste mode. Without that mode,
newlines become carriage returns, as if entered at the terminal. Control bytes
such as Escape are replaced with spaces to prevent pasted escape sequences from
ending bracketed paste early. Pasting into an exited terminal does nothing.
