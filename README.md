# jf-ripper

Rips Blu-rays and DVDs and transcodes them to MKV/MP4, either locally or
distributed across several machines via a shared directory.

## Building

### Dependencies

Besides a Rust toolchain (edition 2024, so Rust 1.85+), the build needs the
following system libraries and tools:

| Purpose                     | Arch Linux      | Debian / Ubuntu                                                                                  |
|-----------------------------|-----------------|--------------------------------------------------------------------------------------------------|
| FFmpeg (`ffmpeg-next`)      | `ffmpeg`        | `libavcodec-dev libavformat-dev libavutil-dev libavdevice-dev libavfilter-dev libswscale-dev`    |
| Blu-ray reading             | `libbluray`     | `libbluray-dev`                                                                                  |
| AACS decryption             | `libaacs`       | `libaacs-dev`                                                                                    |
| BD+ decryption              | `libbdplus`     | `libbdplus-dev`                                                                                  |
| DVD reading                 | `libdvdread`    | `libdvdread-dev`                                                                                 |
| DVD CSS decryption (runtime) | `libdvdcss`     | not in Debian main, build from source (see `Dockerfile`)                                        |
| TLS for `reqwest`           | `openssl`       | `libssl-dev`                                                                                     |
| Bindings generation         | `clang pkgconf` | `clang libclang-dev pkg-config`                                                                  |

Arch:

```sh
sudo pacman -S --needed rust clang pkgconf ffmpeg libbluray libaacs libbdplus libdvdread libdvdcss openssl
```

The `ffmpeg-next` crate's major version tracks FFmpeg's. After a system FFmpeg
upgrade to a new major version (e.g. 8 → 9) the build fails with
non-exhaustive `match` errors in `ffmpeg-next`, so bump the crate to match.
Newer crate versions still build against older FFmpeg (the Docker image builds
`ffmpeg-next` 9 against Debian's FFmpeg 7.1).

libdvdcss is loaded at runtime via `dlopen`. Without it, the build succeeds,
but CSS-encrypted DVDs produce corrupt rips (a warning is printed).

### Build / install

```sh
cargo build --release
cargo install --path .   # installs `jf-ripper` to ~/.cargo/bin
```

## Decryption keys

The libraries look for their key files in the user's config directory:

| File                                     | Used by   | Purpose                                     |
|------------------------------------------|-----------|---------------------------------------------|
| `~/.config/aacs/KEYDB.cfg`               | libaacs   | AACS keys (processing/device keys, VUKs)    |
| `~/.config/bdplus/vm0/memory.map`        | libbdplus | Must exist, may be empty                    |
| `~/.config/bdplus/vm0/ecdsa_keys.txt`    | libbdplus | Must exist and be non-empty (see below)     |
| `~/.config/bdplus/convtab/<MK>_*.bin`    | libbdplus | Cached BD+ conversion tables, one per disc  |

Without VM keys, libbdplus can only decrypt BD+ discs that have a cached
conversion table. Tables are matched by the disc's media key (the 32 hex digit
filename prefix, case-insensitive), so the community table packs can be
extracted into `convtab/` as-is. Extract the base pack first, then the updates
in date order.

libbdplus 0.2.0 segfaults in `bdplus_init` when `ecdsa_keys.txt` is missing.
An empty file doesn't help (it is rejected as too small), so give it a comment
line:

```sh
mkdir -p ~/.config/bdplus/vm0 ~/.config/bdplus/convtab
touch ~/.config/bdplus/vm0/memory.map
echo '# placeholder' > ~/.config/bdplus/vm0/ecdsa_keys.txt
```

Check with `jf-ripper disc-info`. It should report `AACS handled: true` and,
for BD+ discs, `BD+ handled: true`. The cached table is only loaded once a
title is opened (look for `loading cached conversion table` on stderr during
`read-title` / `rip`).

The `BD-J check: Failed to load JVM library` warning is harmless. It only
concerns BD-J disc menus, which aren't needed for ripping.

## Docker

```sh
docker build -t jf-ripper .
```

The container runs as root, so libaacs and libbdplus look for their files under
`/root/.config`, and the config is read from
`/root/.config/bluray-rip/config.toml`. None of these are baked into the
image, so mount them at runtime, along with the drive and the output
directories:

```sh
docker run --rm -it \
  --device /dev/sr0 \
  -v ~/.config/aacs:/root/.config/aacs:ro \
  -v ~/.config/bdplus:/root/.config/bdplus:ro \
  -v ~/.config/bluray-rip-docker/config.toml:/root/.config/bluray-rip/config.toml:ro \
  -v ~/smb/Videos:/videos \
  -v /var/tmp:/var/tmp \
  jf-ripper rip
```

- `--device /dev/sr0` is only needed on the machine with the drive. A pure
  transcoding worker (`jf-ripper worker`) needs neither the device nor the
  key mounts.
- Paths in the mounted config are resolved inside the container, so they must
  use the container-side mount points, not the host paths. Use a separate config
  for Docker, e.g.:

  ```toml
  movie_dir = "/videos/Movies"
  show_dir = "/videos/Shows"
  temp_dir = "/var/tmp"

  [distributed]
  shared_dir = "/videos/jobs"
  ```

- `temp_dir` holds the raw title during a rip (tens of GB for a Blu-ray), so
  mount it from a disk with enough free space rather than the container's
  writable layer.
- `rip` is interactive and needs `-it`. `worker` can run detached.
- Debian ships the same libbdplus 0.2.0, so the `ecdsa_keys.txt` workaround
  above applies inside the container too. It's covered by mounting the host's
  `~/.config/bdplus`.
