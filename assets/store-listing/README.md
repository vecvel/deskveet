# Store listings

Everything a store asks for, one folder per store. The screenshots are the real app with example data (no one's
real files), framed with a caption and built from the app's own stylesheets.

| Folder | For | What is in it |
|---|---|---|
| `desktop/` | the website, GitHub, press, and any store with the usual fields | `listing.md` (name, subtitle, description, keywords, captions) and the screenshots: `screenshots/` at 2880 x 1800, `screenshots-1920/` at 1920 x 1080. No platform is named. |
| `mac-app-store/` | the Mac App Store | `en-US/`: six 2880 x 1800 screenshots with Mac wording. The text is in `fastlane/metadata`; `fastlane mac store_metadata` uploads both. Apple does not allow other platforms to be mentioned there. |
| `microsoft-store/` | the Microsoft Store (Partner Center) | `listing.md` with every field and its limit, ready to paste; screenshots come from `desktop/screenshots-1920/`. |
| `flathub/` | Flathub, and the software centres that read AppStream (GNOME Software, KDE Discover) | `io.github.vecvel.Deskveet.metainfo.xml` and the desktop entry. Flathub needs an id on a domain you control, so it uses the GitHub one. A Flathub submission also needs a build manifest in Flathub's own repository. |

The Snap Store takes the same summary and description as `desktop/listing.md`.

## Screenshots

| # | File | Mac App Store caption | Everywhere else |
|---|---|---|---|
| 1 | `01-storage.png` | See where your space went. | the same |
| 2 | `02-rules.png` | Every rule says what happens. (58 built-in rules) | the same, without a count |
| 3 | `03-review.png` | You see it first. It goes to the Trash. | You see it first. Then it's cleaned. |
| 4 | `04-undo.png` | Changed your mind? Put it back. | the same |
| 5 | `05-overview.png` | Your Mac at a glance. | Your computer at a glance. |
| 6 | `06-private.png` | Small, quiet and private. | the same, with the tray instead of the menu bar |

Every image is an RGB PNG with no transparency, which every store accepts.
