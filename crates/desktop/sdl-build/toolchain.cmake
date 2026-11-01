# Read by SDL3's cmake build (sdl3-sys's build-from-source), through
# CMAKE_TOOLCHAIN_FILE in .cargo/config.toml: README.md beside this.
#
# SDL's X11 driver loads libX11 and its extensions with dlopen, so to build
# it needs their headers, and their libraries only to learn their sonames.
# A machine without the -devel packages has the run-time libraries
# (libXext.so.6 and so on) but not the unversioned .so that SDL looks for,
# and SDL fails the build over an extension it can't find rather than going
# without it. So: each library SDL would look for and not find is pointed
# at its run-time copy, and each extension whose headers are missing is
# turned off. libXext's own two headers, which SDL can't do without, are in
# include/ beside this.
# All of it is for Linux (glibc, X11); elsewhere this file does nothing.
if(CMAKE_HOST_UNIX AND NOT CMAKE_HOST_APPLE)
    set(_gbaa_inc /usr/include /usr/local/include
        "${CMAKE_CURRENT_LIST_DIR}/include")
    set(_gbaa_lib /usr/lib64 /usr/lib /usr/lib/x86_64-linux-gnu /usr/lib/aarch64-linux-gnu
        /usr/local/lib)
    # option | headers (comma-separated) | SDL's variable for the library | its soname
    foreach(_gbaa_ext IN ITEMS
            "|X11/extensions/Xext.h|XEXT_LIB|libXext.so.6"
            "XCURSOR|X11/Xcursor/Xcursor.h|XCURSOR_LIB|libXcursor.so.1"
            "XINPUT|X11/extensions/XInput2.h|XI_LIB|libXi.so.6"
            "XFIXES|X11/extensions/Xfixes.h,X11/extensions/XInput2.h|XFIXES_LIB|libXfixes.so.3"
            "XRANDR|X11/extensions/Xrandr.h|XRANDR_LIB|libXrandr.so.2"
            "XSCRNSAVER|X11/extensions/scrnsaver.h|XSS_LIB|libXss.so.1"
            "XTEST|X11/extensions/XTest.h|XTST_LIB|libXtst.so.6"
            "XSHAPE|X11/extensions/shape.h||"
            "XDBE|X11/extensions/Xdbe.h||"
            "XSYNC|X11/extensions/sync.h||")
        string(REPLACE "|" ";" _gbaa_parts "${_gbaa_ext}")
        list(GET _gbaa_parts 0 _gbaa_opt)
        list(GET _gbaa_parts 1 _gbaa_headers)
        list(GET _gbaa_parts 2 _gbaa_var)
        list(GET _gbaa_parts 3 _gbaa_soname)
        set(_gbaa_have TRUE)
        string(REPLACE "," ";" _gbaa_headers "${_gbaa_headers}")
        foreach(_gbaa_h IN LISTS _gbaa_headers)
            find_file(_gbaa_found NAMES "${_gbaa_h}" PATHS ${_gbaa_inc} NO_CACHE
                NO_DEFAULT_PATH)
            if(NOT _gbaa_found)
                set(_gbaa_have FALSE)
            endif()
            unset(_gbaa_found)
        endforeach()
        # Unset or NOTFOUND (a cache from a configure before this file):
        # looked for again either way.
        if(_gbaa_var AND NOT ${_gbaa_var})
            string(REGEX REPLACE "^lib(.*)\\.so\\..*$" "\\1" _gbaa_name "${_gbaa_soname}")
            find_library(_gbaa_found NAMES "${_gbaa_name}" PATHS ${_gbaa_lib} NO_CACHE
                NO_DEFAULT_PATH)
            if(NOT _gbaa_found)
                find_file(_gbaa_found NAMES "${_gbaa_soname}" PATHS ${_gbaa_lib} NO_CACHE
                    NO_DEFAULT_PATH)
                if(_gbaa_found)
                    set(${_gbaa_var} "${_gbaa_found}" CACHE FILEPATH "The run-time library"
                        FORCE)
                else()
                    set(_gbaa_have FALSE)
                endif()
            endif()
            unset(_gbaa_found)
        endif()
        if(_gbaa_opt AND NOT _gbaa_have)
            set(SDL_X11_${_gbaa_opt} OFF CACHE BOOL "No development files for it here")
        endif()
    endforeach()

    # SteamOS's glibc is older than a build machine's may be: nothing in SDL
    # may need a glibc newer than scripts/deck-package.sh's GLIBC_MAX. These
    # came in later (strlcpy and its kin in 2.38; new versions of these four
    # float functions in 2.43); SDL uses its own instead. glibc-compat.h keeps
    # strtol, sscanf and the like on their old versions.
    foreach(_gbaa_fn STRLCAT STRLCPY WCSLCAT WCSLCPY ACOSF ASINF ATAN2F LOG10F)
        set(LIBC_HAS_${_gbaa_fn} "" CACHE INTERNAL "Newer than SteamOS's glibc")
    endforeach()
    add_compile_options(
        "$<$<COMPILE_LANGUAGE:C>:SHELL:-include ${CMAKE_CURRENT_LIST_DIR}/glibc-compat.h>")
endif()
