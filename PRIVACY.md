# Privacy policy

Last updated 23 October 2026.

Deskveet does not collect, send or share any data. It has no network code, no analytics, no crash reporting and
no accounts, and its content security policy blocks every network connection. Nothing you do in the app leaves
your Mac.

## What stays on your Mac

To do its job Deskveet reads the sizes, names and dates of files in the folders you scan, the list of running
apps and processes, and the apps installed in Applications. When you look for duplicate files it also reads
their contents to compare them; nothing about them is kept after the app quits. It keeps a few files of its own, only on your Mac:

| Where | What |
|---|---|
| `~/Library/Application Support/app.deskveet.desktop` | your settings, the undo log of what was moved to the Trash, the size history per rule, and any custom rules you add |
| `~/Library/Caches/app.deskveet.desktop` | the results of your last scans, so the next one is faster |

Deleting those two folders removes everything Deskveet has stored. Items it cleans go to your Trash, where they
stay until you empty it.

## Permissions

If you give Deskveet Full Disk Access, it uses it only to measure and clean the folders its rules cover. Alerts
and scheduled cleans use macOS notifications, which are delivered on your Mac. Restart and Shut down in the menu
bar ask System Events to do it, which macOS asks you to allow once. Cleanup tools that a rule asks you
to run yourself, such as `brew` or `npm`, are separate programs with their own behaviour.

## Contact

Questions about this policy can be raised at https://github.com/vecvel/deskveet/issues.
