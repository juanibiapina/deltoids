# A method appended to the end of a Python class keeps its own hunk

## Why this case exists

A Python class has no closing line, so appending a method moves the
class's last line onto the new method's last line. The class boundary
then sits inside the insert even though the class started before it.

An insert counts as a new scope only when every structure boundary
inside it belongs to a structure wholly inside it. The class's last line
is also the new method's last line, so the shared boundary must not turn
a clean new method into an edit of the class (see case 058 for the
boundary that does count).

## Behaviours pinned

- One hunk anchored on the new method, under its class.
- The hunk contains only the added lines: no context from `get`.
