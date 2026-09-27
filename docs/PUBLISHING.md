# Publishing Glance

How to get Glance into Flathub, the AUR, and the Fedora, Debian and Ubuntu
archives — and, honestly, how far each of those is actually in your gift.

## Read this first

Two facts decide most of what follows.

### The name `glance` is already taken in three of the five places

| Where | `glance` | Who has it | What Glance must be called |
|---|---|---|---|
| Flathub | free | — | `io.github.abhilesh1412.Glance` |
| AUR | **taken** | a self-hosted dashboard ([glanceapp/glance](https://github.com/glanceapp/glance)) | `glance-image-viewer` |
| Fedora | free | only `glances`, the monitoring tool | `glance` |
| Debian | **taken** | OpenStack Image Service | `glance-image-viewer` |
| Ubuntu | **taken** | same, synced from Debian | `glance-image-viewer` |

The AUR one is worse than a name clash: that package installs `/usr/bin/glance`
as well, so the two cannot coexist. The PKGBUILD here declares
`conflicts=('glance')`, which is the correct way to say so — pacman then refuses
to install both rather than silently overwriting a file.

So `sudo dnf install glance` can work, but on Arch and Debian it will be
`glance-image-viewer`.

### Only two of the five are yours to publish

| Target | Who decides | Realistic time | You can do it alone? |
|---|---|---|---|
| **Flathub** | Flathub reviewer | days to ~2 weeks | **Yes** |
| **AUR** (`yay`) | nobody — self-service | under an hour | **Yes** |
| Arch repos (`pacman`) | an Arch Package Maintainer | indefinite | No |
| Fedora (`dnf`) | a Fedora sponsor + reviewer | weeks to months | No |
| Debian / Ubuntu (`apt`) | a Debian Developer | months | No |

There is no form to fill in for `pacman`, `dnf` and `apt`. Those archives are
curated: a human with upload rights has to agree to carry your package and keep
carrying it. You cannot push to them, and no amount of correct packaging skips
that step.

What you *can* do today, and what gives most people a one-click or one-command
install, is **Flathub and the AUR**. Do those first. The rest are worth starting
in parallel, because they are slow rather than hard.

---

## 1. Flathub

The big one: one build, every distribution, and an Install button.

### Before you start

- The repository must be public, with the licence in it. ✅ Done.
- The metainfo must validate and its screenshots must be reachable over HTTP.
  ✅ Done — they are in `data/screenshots/` and served from GitHub.
- There must be a tagged release, and the manifest must build from a **commit
  hash**, not a tag. A tag can be moved; Flathub needs the build reproducible.

### Step 1 — Tag and release

```bash
git tag -a v1.0.0 -m "Glance 1.0.0"
git push origin v1.0.0
git rev-parse v1.0.0^{commit}      # note this hash, you need it next
```

Then on GitHub: **Releases → Draft a new release → choose `v1.0.0` → Publish**.

### Step 2 — Point the manifest at that commit

Open `build-aux/io.github.abhilesh1412.Glance.yaml`. Replace the `type: dir`
source block with the `type: git` one that is sitting commented out just below
it, and paste in the hash from step 1:

```yaml
      - type: git
        url: https://github.com/ABHILESH1412/glance.git
        tag: v1.0.0
        commit: <the hash>
      - cargo-sources.json
```

### Step 3 — Build it exactly the way Flathub will, and lint it

```bash
flatpak install flathub org.flatpak.Builder
flatpak run org.flatpak.Builder --force-clean --sandbox --user --install \
    --install-deps-from=flathub --ccache --mirror-screenshots-url=https://dl.flathub.org/media/ \
    --repo=repo builddir build-aux/io.github.abhilesh1412.Glance.yaml

flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest \
    build-aux/io.github.abhilesh1412.Glance.yaml
flatpak run --command=flatpak-builder-lint org.flatpak.Builder repo repo
```

Both lint runs must come back clean. Fix anything they report before submitting —
a reviewer will only run the same commands.

### Step 4 — Open the submission PR

The submission goes to the `flathub/flathub` repository, on the **`new-pr`**
branch. Not `master` — a PR against `master` gets closed.

```bash
# Fork github.com/flathub/flathub first, with "copy the master branch only"
# UNCHECKED, or the new-pr branch will not exist in your fork.
git clone --branch=new-pr git@github.com:<your-username>/flathub.git
cd flathub
git checkout -b glance-submission new-pr

cp ~/work/glance/build-aux/io.github.abhilesh1412.Glance.yaml .
cp ~/work/glance/build-aux/cargo-sources.json .

git add io.github.abhilesh1412.Glance.yaml cargo-sources.json
git commit -m "Add io.github.abhilesh1412.Glance"
git push -u origin glance-submission
```

Open the pull request **against the `new-pr` branch**, titled:

```
Add io.github.abhilesh1412.Glance
```

### Step 5 — Review

A bot builds your app and comments. A human reviews after that. Expect questions
about `--filesystem=host` — the answer is in the manifest's comments: a folder
browsing image viewer needs the folder, and the document portal hands over one
file at a time. Eye of GNOME has the same permission for the same reason.

### After it is merged

You get a repository at `github.com/flathub/io.github.abhilesh1412.Glance` and
write access to it. **Every future release goes there, not through this process
again** — edit the manifest's commit hash in that repo, push, and the build
happens automatically.

Users then get: **flathub.org → Install.** Nothing else.

---

## 1b. When you cannot wait: ship the package yourself

The AUR closes new registrations from time to time, and Flathub review takes as
long as it takes. Neither has to stop you handing someone a working install
today, because pacman will install a package straight from a URL:

```bash
sudo pacman -U https://github.com/ABHILESH1412/glance/releases/download/v1.0.0/glance-image-viewer-1.0.0-1-x86_64.pkg.tar.zst
```

That one line downloads it, pulls `gtk4`, `libadwaita` and `libheif` from the
official repositories if they are missing, installs the binary, the desktop
entry, the icons and the metainfo, and refreshes the icon and desktop caches.
Afterwards Glance is in the launcher and in "Open With" like anything else, and
`sudo pacman -R glance-image-viewer` removes it cleanly.

### Making the file

```bash
cd packaging/aur
updpkgsums          # once the v1.0.0 tag exists
makepkg -f
ls *.pkg.tar.zst    # this is what you upload
```

Attach that `.pkg.tar.zst` to the GitHub release alongside the source, and the
command above works for anyone on Arch.

### What it does not do

- **No automatic updates.** A package installed this way is invisible to
  `pacman -Syu`; the user has to run the command again for 1.0.1. The AUR and a
  hosted repository both solve that, this does not.
- **x86_64 only**, unless you build on an ARM machine as well.
- **Arch only.** Fedora and Debian would need an `.rpm` and a `.deb`, built on
  those distributions.
- **Unsigned.** The user is trusting a GitHub URL. That is the same trust they
  extend to a PKGBUILD, but it is worth being honest that it is trust, not
  verification.

If you would rather users got updates, the next step up is a small pacman
repository of your own — a directory of packages plus a `.db` made by
`repo-add`, hosted anywhere static, and three lines added to `pacman.conf`.
That costs the user one bit of setup and then behaves exactly like any official
repository, upgrades included.

**Do not** hand people a `curl ... | sudo bash` line. It runs whatever the URL
happens to serve at that moment, with root, unread. A signed package that
pacman can list, verify and remove is strictly better, and you already have one.

## 2. The AUR — `yay -S glance-image-viewer`

The fastest real win. No review, no gatekeeper; you push and it is live.

> **If registration is closed.** The AUR periodically pauses new accounts to
> fend off automated sign-ups; the page returns HTTP 503 and says so. There is
> no queue and no way to ask for an exception. Watch
> [the Arch news feed](https://archlinux.org/news/) or the `aur-general` list,
> and in the meantime use the release-asset route in section 1b — it gets Arch
> users a working install with one command. None of this affects Flathub, which
> needs only a GitHub account.

### Step 0 — The two tools you do not have yet

```bash
sudo pacman -S --needed base-devel pacman-contrib namcap
```

`pacman-contrib` provides `updpkgsums`, which fills in the source checksum;
`namcap` lints both the recipe and the built package.

### Step 1 — An AUR account with an SSH key

Register at [aur.archlinux.org/register](https://aur.archlinux.org/register),
then add your public key under **My Account → SSH Public Key**:

```bash
ssh-keygen -t ed25519 -C "aur"      # if you do not have one
cat ~/.ssh/id_ed25519.pub           # paste this into the form
ssh aur@aur.archlinux.org help      # should greet you by username
```

### Step 2 — Fill in the release checksum

`packaging/aur/PKGBUILD` ships with `sha256sums=('SKIP')`, which is fine for
testing and **not** acceptable for a real package. After the v1.0.0 tag exists:

```bash
cd packaging/aur
updpkgsums          # from pacman-contrib; downloads the tarball and fills it in
```

### Step 3 — Generate `.SRCINFO` and push

The AUR will reject a push without `.SRCINFO`, and it must match the PKGBUILD.

```bash
git clone ssh://aur@aur.archlinux.org/glance-image-viewer.git ~/aur-glance
cd ~/aur-glance
cp ~/work/glance/packaging/aur/PKGBUILD .
makepkg --printsrcinfo > .SRCINFO

git add PKGBUILD .SRCINFO
git commit -m "Initial release: glance-image-viewer 1.0.0"
git push
```

That is it — it is live immediately.

### Step 4 — Check it the way the AUR does

```bash
makepkg -si          # builds and installs, from a clean checkout
namcap PKGBUILD      # lints the recipe
namcap *.pkg.tar.zst # lints the built package
```

Users then run:

```bash
yay -S glance-image-viewer
```

### Every new version

Bump `pkgver`, reset `pkgrel=1`, `updpkgsums`, regenerate `.SRCINFO`, push.

---

## 3. The Arch repositories — `pacman -S`

**Not something you can submit to.** `extra` is maintained by Arch Package
Maintainers, and packages get there because a maintainer decides to adopt one,
not because the author asks.

The realistic route is the long way round:

1. Ship it on the AUR and let it collect votes and comments. Popularity is the
   evidence a maintainer looks at.
2. Once it has a real user base, raise it on the `arch-general` mailing list or
   the forums and ask whether a maintainer would consider adopting it.
3. Or become an Arch Package Maintainer yourself — a months-long process
   involving sponsorship by two existing maintainers.

Until then, `yay -S glance-image-viewer` is the Arch answer, and for most Arch
users that is completely normal. Do not hold up the other four waiting for this.

---

## 4. Fedora — `dnf install glance`

Two routes. Take both: COPR now, official later.

### Route A — COPR, today, self-service

COPR is Fedora's build-and-host service for anyone with a FAS account. It gives
users a working `dnf install` at the cost of one extra command to add the repo.

1. Create a [Fedora account](https://accounts.fedoraproject.org/).
2. Go to [copr.fedorainfracloud.org](https://copr.fedorainfracloud.org/),
   **New Project** → name it `glance`, tick the Fedora releases and
   architectures you want.
3. Make the vendored crate tarball the spec expects, and build a source RPM:

```bash
cd ~/work/glance
cargo vendor
tar caf glance-1.0.0-vendor.tar.xz vendor/
# Upload both the release tarball and the vendor tarball, or point COPR at a
# dist-git style repo containing packaging/fedora/glance.spec
```

4. **Builds → New Build → Upload** the SRPM, or connect the GitHub repo so each
   tag builds automatically.

Users then run:

```bash
sudo dnf copr enable <your-fas-name>/glance
sudo dnf install glance
```

### Route B — the official Fedora archive

This is what makes plain `sudo dnf install glance` work, with no repo to add.

1. **Fedora account**, then join the `packager` group — which requires a
   sponsor, someone already in the group who vouches for your work.
2. **File a package review request** in Bugzilla under *Fedora → Package
   Review*, attaching the `.spec` and the SRPM URL.
3. A reviewer goes through it against the
   [packaging guidelines](https://docs.fedoraproject.org/en-US/packaging-guidelines/).
   For Rust they will expect either `rust2rpm` output or a clear justification
   for bundled crates, and a `Provides: bundled(crate(...))` list. Running
   `rust2rpm` on the project first will save you a round of review:

```bash
sudo dnf install rust2rpm
rust2rpm -t fedora .
```

4. Once the review passes, you request the dist-git repository, import, and
   build in Koji. It lands in the next Fedora release, and in updates for the
   current one via Bodhi.

Budget several weeks, most of it waiting for a reviewer. The `.spec` in
`packaging/fedora/` is the starting point, not the finished article — expect the
reviewer to want changes.

---

## 5. Debian and Ubuntu — `apt install`

The slowest of the five, and the one where the name definitely changes.

### Route A — a Launchpad PPA, today (Ubuntu only)

```bash
# Register at launchpad.net, upload a GPG key and an SSH key, create a PPA.
sudo apt install devscripts dput debhelper
cd ~/work/glance
cp -r packaging/debian debian
cargo vendor                          # Debian builders have no network
debuild -S -sa                        # builds a source package and signs it
dput ppa:<your-launchpad-name>/glance ../glance-image-viewer_1.0.0-1_source.changes
```

Users then run:

```bash
sudo add-apt-repository ppa:<your-launchpad-name>/glance
sudo apt update && sudo apt install glance-image-viewer
```

### Route B — Debian proper, which Ubuntu then inherits

1. **File an ITP** (Intent To Package) bug, so nobody duplicates your work:

```bash
sudo apt install reportbug
reportbug --email you@example.com wnpp
# choose ITP, package glance-image-viewer
```

Put that bug number in `debian/changelog` where it says `#NNNNNN`.

2. **Build and check it** until both are silent:

```bash
debuild -us -uc
lintian -EviIL +pedantic ../glance-image-viewer_1.0.0-1_amd64.changes
```

3. **Upload to [mentors.debian.net](https://mentors.debian.net/)** and file an
   **RFS** (Request For Sponsorship) bug against `sponsorship-requests`.

4. **Wait for a Debian Developer** to review and upload it. This is the step with
   no deadline. Chasing it politely on `debian-mentors` is normal and expected.

5. Once in Debian testing, Ubuntu picks it up automatically at the next import,
   and `apt install glance-image-viewer` works there too.

Rust packaging in Debian has its own rules — `dh-cargo`, and a strong preference
for crates packaged separately rather than vendored. For an application with a
large dependency tree, vendoring with a clear `debian/copyright` is usually
accepted, but expect discussion.

---

## Where that leaves you

| Command a user types | Works after |
|---|---|
| `sudo pacman -U <release URL>` | you upload one file — **today** |
| Install button on flathub.org | Flathub PR merged — days |
| `yay -S glance-image-viewer` | you push to the AUR — today, when registration reopens |
| `sudo dnf copr enable you/glance && sudo dnf install glance` | a COPR build — today |
| `sudo add-apt-repository ppa:you/glance && sudo apt install glance-image-viewer` | a PPA upload — today |
| `sudo dnf install glance` | Fedora review passes — weeks |
| `sudo apt install glance-image-viewer` | a Debian Developer sponsors it — months |
| `sudo pacman -S ...` | an Arch maintainer adopts it — indefinite |

Flathub gets you to "no manual work" for the great majority of Linux users, and
it is not affected by the AUR being closed. Do that one. The release asset in
section 1b covers Arch users in the meantime, in a single command. The rest is
patience, not engineering.
