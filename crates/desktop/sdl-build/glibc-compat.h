/* Included before every C file of SDL3's build (toolchain.cmake beside
 * this), so the binary runs on an older glibc than the one it's built on:
 * SteamOS's (scripts/deck-package.sh's GLIBC_MAX). glibc 2.38 and later
 * send strtol, sscanf and the like to new __isoc23_ versions whenever
 * _GNU_SOURCE is on, as SDL has it; this keeps the old ones, which differ
 * only in reading C23's 0b numbers. */
#ifndef GBAA_GLIBC_COMPAT_H
#define GBAA_GLIBC_COMPAT_H
#ifdef __linux__
#ifndef _GNU_SOURCE
#define _GNU_SOURCE 1
#endif
#include <features.h>
#ifdef __GLIBC__
#undef __GLIBC_USE_C23_STRTOL
#define __GLIBC_USE_C23_STRTOL 0
#endif
#endif
#endif
