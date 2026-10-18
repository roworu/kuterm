# Release

merging a `release/v*` pull request into `main` runs `.github/workflows/release.yml`. it builds linux, macos and windows binaries, then publishes a github release with checksums and generated notes. the release creates the `v*` tag itself, tests already ran on the pull request.

1. create the branch from up to date `main`:
   ```bash
   VERSION="v0.0.2"
   git switch main && git pull
   git switch -c "release/$VERSION"
   ```
2. bump `version` in `Cargo.toml` and update `Cargo.lock` (no build, so it works without native deps):
   ```bash
   sed -i "0,/^version = .*/s//version = \"${VERSION#v}\"/" Cargo.toml
   cargo update --workspace
   ```
3. commit and push, then open the pull request into `main`:
   ```bash
   git commit -am "release $VERSION"
   git push -u origin "release/$VERSION"
   gh pr create --base main --head "release/$VERSION"
   ```
4. merge the pull request; the merge starts the release. watch it, then check the release:
   ```bash
   gh run watch
   gh release view "$VERSION"
   ```

to redo a release, delete it and its tag first:
`gh release delete "$VERSION" --cleanup-tag`
