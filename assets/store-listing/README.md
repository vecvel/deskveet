# Store listing

Six screenshots for the Mac App Store listing, in `en-US/`. Each is 2880 x 1800 (16:10), the largest size App
Store Connect accepts for macOS. It also accepts 1280 x 800, 1440 x 900 and 2560 x 1600, and scales these down
for smaller displays. The images are RGB PNGs with no transparency, as Apple requires.

`fastlane mac store_metadata` uploads this folder together with the text in `fastlane/metadata`.

| # | File | Caption | Supporting line |
|---|---|---|---|
| 1 | `01-storage.png` | See where your space went. | A fast scan of any folder, shown as a treemap you can scroll through. |
| 2 | `02-rules.png` | Every rule says what happens. | 56 built-in rules. Each one tells you what you lose and how it comes back. |
| 3 | `03-review.png` | You see it first. It goes to the Trash. | Nothing is cleaned until you have looked at the list and said yes. |
| 4 | `04-custom-rules.png` | Add your own rules. | Type a path, see what it matches right away. Custom rules only ever move things to the Trash. |
| 5 | `05-overview.png` | Your Mac at a glance. | Disks, memory pressure and CPU, live while you look and silent when you don't. |
| 6 | `06-private.png` | Small, quiet and private. | No network code, no tracking, no accounts. Just a menu bar item when you want one. |

The screens are built from the app's own stylesheets (`src/styles`), so they follow the same tokens as the app:
system font, the warm dark palette, one near-transparent border and the darker filled controls. Folder names and
sizes are examples, not anyone's real files.
