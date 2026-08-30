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
# This exits non-zero when the guest's self-test fails, when it never
# reports at all, or when the boot hangs. That is not incidental: a
# harness that ends by `exec`-ing qemu reports qemu's exit status, and a
# result printed to the serial console is something a human reads and CI
# does not — so wiring one up unchanged produces a step that passes
# unconditionally. The output is captured, the exit line is grepped for,
# and a missing one is a failure.
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

cargo build --features fbdev --example selftest --example probe

R=$WORK/root
mkdir -p "$R/bin" "$R/lib/x86_64-linux-gnu" "$R/lib64" "$R/proc" "$R/sys" "$R/dev"
cp target/debug/examples/selftest target/debug/examples/probe "$R/"
BUSYBOX=$(command -v busybox)
cp "$BUSYBOX" "$R/bin/"
ln -s busybox "$R/bin/sh"
# The examples are dynamically linked; carry just their libs. `|| true`
# because a statically linked busybox — which is what `busybox-static`
# installs, and what CI uses — makes ldd exit non-zero after saying so,
# and that is a normal outcome here rather than a failure.
for lib in $( (ldd "$BUSYBOX" target/debug/examples/selftest || true) |
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
echo "### PROBE"
/probe /dev/fb0 2>&1
echo "PROBE_EXIT=$?"
echo "### SELFTEST"
/selftest /dev/fb0 2>&1
echo "SELFTEST_EXIT=$?"
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
# packing three channels on top of each other, so `probe` failing here is
# the result being asserted.
EXPECT_REFUSAL=no
case "$MODE" in
    0x303|0x305|0x307|0x30[dD]) EXPECT_REFUSAL=yes ;;
esac

exit_line() {
    sed 's/\r$//' "$LOG" | grep "^$1=" | tail -n 1 | cut -d= -f2
}
PROBE=$(exit_line PROBE_EXIT)
SELFTEST=$(exit_line SELFTEST_EXIT)

if [ "$EXPECT_REFUSAL" = yes ]; then
    if [ -z "$PROBE" ]; then
        echo "FAIL: guest never reported a probe result" >&2
        exit 1
    fi
    if [ "$PROBE" = 0 ]; then
        echo "FAIL: vga=$MODE is a palette framebuffer and was accepted" >&2
        exit 1
    fi
    echo "PASS: vga=$MODE refused, as it must be"
    exit 0
fi

if [ -z "$SELFTEST" ]; then
    echo "FAIL: guest never reported a self-test result" >&2
    exit 1
fi
if [ "$SELFTEST" != 0 ]; then
    echo "FAIL: self-test exited $SELFTEST" >&2
    exit 1
fi
echo "PASS: vga=$MODE"
