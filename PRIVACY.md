# Privacy policy

Last updated 1 November 2026.

Deskveet does not collect, send or share any data. It has no analytics, no crash reporting and no accounts, and
its window's content security policy blocks every network connection. Nothing you do in the app leaves your computer.

## Updates

Deskveet's only connection is to GitHub, to keep itself up to date. Soon after it starts and then once a day it
downloads a small file from `github.com/vecvel/deskveet/releases` that names the latest version, and when there is
a newer one it downloads that too. The request carries nothing about you, your files or how you use the app; as
with any download, GitHub sees your IP address. Every update is checked against Deskveet's signature before it is
used. You can turn automatic updates off in Settings → Privacy, and then Deskveet makes no connection at all.

## What stays on your computer

To do its job Deskveet reads the sizes, names and dates of files in the folders you scan, the list of running
apps and processes, and the apps installed in Applications. When you look for duplicate files it also reads
their contents to compare them; nothing about them is kept after the app quits. It keeps a few files of its own, only on your Mac:

| Where | What |
|---|---|
| `~/Library/Application Support/app.deskveet.desktop` | your settings, the undo log of what was moved to the Trash, the size history per rule, and any custom rules you add |
| `~/Library/Caches/app.deskveet.desktop` | the results of your last scans, so the next one is faster |

On Windows the same files are in `%APPDATA%\app.deskveet.desktop` and `%LOCALAPPDATA%\app.deskveet.desktop\cache`
(the cache also holds a downloaded update until it is installed), and cleaned items go to the Recycle Bin. On
Linux they are in `~/.local/share/app.deskveet.desktop` and `~/.cache/app.deskveet.desktop`, and cleaned items go to
the Trash.

Deleting those two folders removes everything Deskveet has stored. Items it cleans go to your Trash, where they
stay until you empty it.

## Permissions

If you give Deskveet Full Disk Access, it uses it only to measure and clean the folders its rules cover. Alerts
and scheduled cleans use macOS notifications, which are delivered on your Mac. Restart and Shut down in the menu
bar ask System Events to do it, which macOS asks you to allow once. Cleanup tools that a rule asks you
to run yourself, such as `brew` or `npm`, are separate programs with their own behaviour.

On Windows Deskveet needs no administrator. It reads the list of installed apps and startup apps from the
registry, writes one value there only when you turn on "Open at sign-in", and runs an app's own uninstaller only
when you ask it to.

## Contact

Questions about this policy can be raised at https://github.com/vecvel/deskveet/issues.
