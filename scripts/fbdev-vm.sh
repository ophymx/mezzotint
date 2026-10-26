#!/bin/sh
# Run the fbdev backend against a framebuffer a real kernel configured.
#
#   scripts/fbdev-vm.sh [vga-mode]
#
# A framebuffer's pixel format comes from hardware, so unit tests can only
# assert constants somebody typed. QEMU supplies real ones. Modes worth
# running:
#
#   0x314  800x600 RGB565    0x317  1024x768 RGB565
#   0x315  800x600 24bpp     0x303  800x600 8bpp palette (must be refused)
#
# Needs qemu-system-x86_64, busybox, cpio, gzip and a readable host kernel.
# Nothing here needs root, and it never touches your own framebuffer.
#
# This exits non-zero when the guest's bring-up self-test fails, when it never
# reports at all, or when the boot hangs. That is not incidental: a
# harness that ends by `exec`-ing qemu reports qemu's exit status, and a
# result printed to the serial console is something a human reads and CI
# does not — so wiring one up unchanged produces a step that passes
# unconditionally. The output is captured, the exit line is grepped for,
# and a missing one is a failure.
#
# The same care is owed to the modes that must be *refused*. "Probe
# exited non-zero" is not evidence the backend declined a palette
# framebuffer: a kernel that binds `simpledrm` rather than `vesafb` will
# not drive an 8bpp palette mode at all, so there is no /dev/fb0, and
# probe fails for a reason that has nothing to do with the rejection
# under test. So the guest reports whether the device existed, `probe`
# exits with a code that says *why* it failed, and this asserts on both.
# Set MUST_REFUSE=yes on a mode that should be accepted to check that
# this gate can still fail.
set -eu

MODE=${1:-0x314}
KERNEL=${KERNEL:-/boot/vmlinuz-$(uname -r)}
TIMEOUT=${TIMEOUT:-120}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

[ -r "$KERNEL" ] || { echo "cannot read $KERNEL — set KERNEL=" >&2; exit 1; }
command -v qemu-system-x86_64 >/dev/null || { echo "qemu-system-x86_64 not found" >&2; exit 1; }
command -v busybox >/dev/null || { echo "busybox not found" >&2; exit 1; }
command -v cpio >/dev/null || { echo "cpio not found" >&2; exit 1; }

cargo build --features fbdev --example bringup --example probe

R=$WORK/root
mkdir -p "$R/bin" "$R/lib/x86_64-linux-gnu" "$R/lib64" "$R/proc" "$R/sys" "$R/dev"
cp target/debug/examples/bringup target/debug/examples/probe "$R/"
BUSYBOX=$(command -v busybox)
cp "$BUSYBOX" "$R/bin/"
ln -s busybox "$R/bin/sh"
# The examples are dynamically linked; carry just their libs. `|| true`
# because a statically linked busybox — which is what `busybox-static`
# installs, and what CI uses — makes ldd exit non-zero after saying so,
# and that is a normal outcome here rather than a failure.
for lib in $( (ldd "$BUSYBOX" target/debug/examples/bringup || true) |
             awk '/=> \//{print $3}' | sort -u); do
    cp "$lib" "$R/lib/x86_64-linux-gnu/"
done
cp /lib64/ld-linux-x86-64.so.2 "$R/lib64/"

cat > "$R/init" <<'INIT'
#!/bin/sh
B=/bin/busybox
$B mount -t proc none /proc
$B mount -t sysfs none /sys
# Stock kernels do not all set CONFIG_DEVTMPFS_MOUNT, so /dev is ours to make.
$B mount -t devtmpfs none /dev
echo "### fb0: $($B cat /sys/class/graphics/fb0/name 2>&1), \
$($B cat /sys/class/graphics/fb0/bits_per_pixel 2>&1) bpp, \
$($B cat /sys/class/graphics/fb0/virtual_size 2>&1)"
if [ -e /dev/fb0 ]; then echo "FB0=present"; else echo "FB0=absent"; fi
echo "### PROBE"
/probe /dev/fb0 2>&1
echo "PROBE_EXIT=$?"
echo "### BRINGUP"
/bringup /dev/fb0 2>&1
echo "BRINGUP_EXIT=$?"
$B poweroff -f
INIT
chmod +x "$R/init"

( cd "$R" && find . | cpio -o -H newc --quiet ) | gzip -9 > "$WORK/initramfs.gz"

echo "=== booting vga=$MODE ==="
LOG=$WORK/console.log
STATUS=0
# `timeout` rather than trusting the guest to power itself off: a kernel
# that panics before init, or an init that wedges, would otherwise hang
# the whole job rather than failing it.
timeout --foreground "$TIMEOUT" qemu-system-x86_64 \
    -kernel "$KERNEL" -initrd "$WORK/initramfs.gz" \
    -append "console=ttyS0 vga=$MODE nomodeset panic=1" \
    -vga std -m 512 -nographic -no-reboot > "$LOG" 2>&1 || STATUS=$?
sed 's/\r$//' "$LOG"

if [ "$STATUS" -eq 124 ]; then
    echo "FAIL: guest did not finish within ${TIMEOUT}s" >&2
    exit 1
fi

# The palette mode is the one case where a refusal is the pass. The
# backend must decline an 8bpp pseudocolour framebuffer rather than
# packing three channels on top of each other, so `probe` failing *for
# that reason* is the result being asserted.
EXPECT_REFUSAL=no
case "$MODE" in
    0x303|0x305|0x307|0x30[dD]) EXPECT_REFUSAL=yes ;;
esac
EXPECT_REFUSAL=${MUST_REFUSE:-$EXPECT_REFUSAL}

reported() {
    sed 's/\r$//' "$LOG" | grep "^$1=" | tail -n 1 | cut -d= -f2
}
FB0=$(reported FB0)
PROBE=$(reported PROBE_EXIT)
BRINGUP=$(reported BRINGUP_EXIT)

if [ -z "$FB0" ]; then
    echo "FAIL: guest never reported whether it had a framebuffer" >&2
    exit 1
fi
if [ "$FB0" != present ]; then
    # Not a skip. A mode this kernel cannot produce a framebuffer for
    # tests nothing, and saying so is the difference between a gate and a
    # green tick — for the refusal modes especially, where "probe failed"
    # would otherwise read as "the backend refused" when what happened is
    # that there was no device to refuse.
    echo "FAIL: no /dev/fb0 for vga=$MODE, so nothing was exercised." >&2
    echo "      A kernel that binds simpledrm rather than vesafb will not" >&2
    echo "      drive every VESA mode; pick one it does, or boot a kernel" >&2
    echo "      with CONFIG_FB_VESA." >&2
    exit 1
fi

if [ "$EXPECT_REFUSAL" = yes ]; then
    if [ -z "$PROBE" ]; then
        echo "FAIL: guest never reported a probe result" >&2
        exit 1
    fi
    # 3 is `probe`'s code for "this backend declines these pixels", which
    # is the only failure that counts as the refusal working. Any other
    # non-zero status is probe failing for an unrelated reason.
    if [ "$PROBE" = 0 ]; then
        echo "FAIL: vga=$MODE is a framebuffer this backend must refuse, and it was accepted" >&2
        exit 1
    fi
    if [ "$PROBE" != 3 ]; then
        echo "FAIL: probe failed for vga=$MODE with status $PROBE, which is not a" >&2
        echo "      refusal of the pixel format — see the console above." >&2
        exit 1
    fi
    echo "PASS: vga=$MODE refused for its pixel format, as it must be"
    exit 0
fi

if [ -z "$BRINGUP" ]; then
    echo "FAIL: guest never reported a bring-up result" >&2
    exit 1
fi
if [ "$BRINGUP" != 0 ]; then
    echo "FAIL: bring-up self-test exited $BRINGUP" >&2
    exit 1
fi
echo "PASS: vga=$MODE"
