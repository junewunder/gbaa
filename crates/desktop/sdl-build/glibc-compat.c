/* Linked whole into every binary with the desktop crate (build.rs), so the
 * binary runs on an older glibc than the one it's built on: SteamOS's
 * (scripts/deck-package.sh's GLIBC_MAX). glibc 2.43 gave these four float
 * functions new, correctly rounded versions, and Rust's f32::atan2 and its
 * kin (egui's among them) link the newest version there is. The binary's
 * own definitions here win over libm's and call the old versions, which
 * SteamOS has. SDL's own build goes without them (toolchain.cmake). */
#ifdef __linux__
#include <features.h>
#endif
#if defined(__linux__) && defined(__GLIBC__) && defined(__x86_64__)
#define GBAA_OLD(fn) __asm__(".symver gbaa_old_" #fn "," #fn "@GLIBC_2.2.5")
float gbaa_old_acosf(float);
float gbaa_old_asinf(float);
float gbaa_old_atan2f(float, float);
float gbaa_old_log10f(float);
GBAA_OLD(acosf);
GBAA_OLD(asinf);
GBAA_OLD(atan2f);
GBAA_OLD(log10f);
float acosf(float x) { return gbaa_old_acosf(x); }
float asinf(float x) { return gbaa_old_asinf(x); }
float atan2f(float y, float x) { return gbaa_old_atan2f(y, x); }
float log10f(float x) { return gbaa_old_log10f(x); }
#endif
