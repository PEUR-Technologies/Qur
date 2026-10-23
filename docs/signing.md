# Signing and notarizing Qur.app

`scripts/bundle.sh` builds `target/release/Qur.app` and signs it **ad hoc** (`codesign --sign -`). That's enough to run it on the Mac that built it. Notifications also work, since macOS only needs a bundle identifier (`ph.peur.qur`) for them. To give the app to other people, sign it with a Developer ID and have Apple notarize it, or Gatekeeper will block it.

## What you need (once)

- An Apple Developer Program membership for PEUR Technologies.
- A **Developer ID Application** certificate in your login keychain (Xcode > Settings > Accounts > Manage Certificates > +). Check with `security find-identity -v -p codesigning`.
- An app-specific password for notarization (appleid.apple.com > Sign-In and Security), stored once:
  ```sh
  xcrun notarytool store-credentials qur-notary --apple-id you@example.com --team-id TEAMID
  ```

## Release build

```sh
SIGN_ID="Developer ID Application: PEUR Technologies (TEAMID)" scripts/bundle.sh --zip
xcrun notarytool submit target/release/Qur.zip --keychain-profile qur-notary --wait
xcrun stapler staple target/release/Qur.app
ditto -c -k --keepParent target/release/Qur.app target/release/Qur.zip   # re-zip with the ticket
spctl --assess --type execute -vv target/release/Qur.app                 # "accepted, source=Notarized Developer ID"
```

`bundle.sh` signs with the hardened runtime (`--options runtime`), which notarization requires. Qur needs no entitlements: it doesn't use the camera, the microphone or the network itself. Agents and the Browser's WebKit run as separate processes, and WebKit has its own entitlements.

## What's in the bundle

| Path | |
|---|---|
| `Contents/MacOS/qur` | The single binary (also `qur doctor`). Grammars, themes, fonts, icons and the chime are compiled in. |
| `Contents/Resources/Qur.icns` | Built by `scripts/make_icns.swift`: the 3D render from `assets/brand/icons/app-icon-3d-1024.png` at 128 px and up, the flat small symbol below 96 px (brand README), on Apple's rounded-tile grid. |
| `Contents/Info.plist` | `ph.peur.qur`, version from `Cargo.toml`, macOS 13 or newer, the developer-tools category. |

## Not done yet

- No auto-update. That's deliberate: it would need a network call, which needs the owner's OK. A new version is a new download.
- No `.dmg`. A zip of the app is enough for now.
