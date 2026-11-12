# Commands in the address bar

With Settings › Files and folders › Behaviors › "Run commands from the address bar" on (the default), the address bar works like
Explorer's: press <kbd>Ctrl</kbd>+<kbd>L</kbd>, type `cmd`, `powershell`, `pwsh`, `wt` or `"C:\Tools\my app\x.exe" /s`
and press <kbd>Enter</kbd>. The text is first taken as a place (a path, a drive, `%VAR%`, `@alias`, `shell:`, `home`); a
folder or file of that name in the open folder always wins. Only when no such place exists is the first word (or the quoted
first part) looked up as a program the way the Run box does (system folders and `PATH`, `App Paths`, app execution aliases
such as `wt`) and started with the rest as its arguments, with the open folder as its working directory (your profile folder
when the tab shows something that is not a folder on disk). The tab stays where it is. Nothing is started from a completion,
from <kbd>Tab</kbd> or from a path that reaches the app any other way, no administrator rights are asked for (a program that
needs them asks by its own prompt), and text that is neither a place nor a program behaves as before. A web address typed
there is not opened: it is treated like any other path that does not exist. A full path to a program typed alone (nothing after
it) names a file that exists, so it is a place and the tab goes to that file; give the program by its name (`cmd`) or add an
argument (`"C:\Tools\my app\x.exe" /s`) to run it. A relative name is read against the open folder, and `..` and `.` are folded
away, as in Explorer.

Not in a release yet: this text moves into the README with the version that ships it.
