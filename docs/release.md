# Releasing Qur

Do these in order for every version. The version lives in one place, `Cargo.toml` (`[workspace.package] version`): the About window, `qur doctor`, the log and the app bundle all read it from there, so a missed bump shows up as the old number in **Qur > About Qur**.

1. **Bump the version** in the root `Cargo.toml`, then build once so `Cargo.lock` follows (`cargo build -p qur_app`).
2. **Check:** `cargo test --workspace` passes and `cargo clippy --workspace --all-targets` adds no warnings.
3. **Record it:** add a numbered entry to `docs/decisions.md` for anything the owner asked for, with their words.
4. **Build the app:** `scripts/bundle.sh --zip` gives `target/release/Qur.app` and `Qur.zip`. Open it and check **About Qur** shows the new version.
5. **Merge:** open a PR into `main` and merge it with a merge commit.
6. **Tag:** `git tag -a vX.Y.Z -m "Qur vX.Y.Z" origin/main && git push origin vX.Y.Z`.
7. **Publish:** `gh release create vX.Y.Z target/release/Qur.zip --title "Qur vX.Y.Z" --notes-file <notes> --verify-tag`. The asset must be named `Qur.zip`: the website's Download button links to `releases/latest/download/Qur.zip`.
8. **Homebrew** (`PEUR-Technologies/homebrew-qur`): in `Casks/qur.rb`, set `version` to the new number and `sha256` to `shasum -a 256 target/release/Qur.zip` (the zip you uploaded), then commit and push. Check with `brew update && brew upgrade --cask qur`. Release notes give the install steps: `brew tap peur-technologies/qur`, `brew trust peur-technologies/qur`, `brew install --cask qur`; and, for the zip, `xattr -dr com.apple.quarantine /Applications/Qur.app` (Qur isn't notarized, so macOS blocks it otherwise; the cask clears that itself).
9. **Website** (`PEUR-Technologies/qursite`): nothing to do for a version bump. Its Download button links to `releases/latest/download/Qur.zip` and reads "Download newest release", so a new release shows up by itself. Update the site only if the app's size changed enough to matter (the size in `weight` / the Light heading in `src/App.tsx`): then `npm run build`, commit, push, and deploy with `npx wrangler pages deploy dist --project-name qursite --branch main` (the Pages project isn't connected to git).
10. **Install locally:** replace `/Applications/Qur.app` with the new build.
