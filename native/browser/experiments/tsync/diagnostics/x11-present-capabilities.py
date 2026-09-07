#!/usr/bin/env python3

import argparse
import ctypes as c
import json
import os
import sys


class Cookie(c.Structure):
    _fields_ = [("sequence", c.c_uint32)]


class Version(c.Structure):
    _fields_ = [
        ("response", c.c_uint8),
        ("pad", c.c_uint8),
        ("sequence", c.c_uint16),
        ("length", c.c_uint32),
        ("major", c.c_uint32),
        ("minor", c.c_uint32),
    ]


class ScreenIterator(c.Structure):
    _fields_ = [("data", c.c_void_p), ("remaining", c.c_int), ("index", c.c_int)]


def query_version(connection, library, prefix, free):
    query = getattr(library, prefix + "_query_version")
    query.argtypes = [c.c_void_p, c.c_uint32, c.c_uint32]
    query.restype = Cookie
    reply = getattr(library, prefix + "_query_version_reply")
    reply.argtypes = [c.c_void_p, Cookie, c.POINTER(c.c_void_p)]
    reply.restype = c.c_void_p
    error = c.c_void_p()
    pointer = reply(connection, query(connection, 1, 4), c.byref(error))
    try:
        if error.value or not pointer:
            raise RuntimeError(prefix + " version query failed")
        value = c.cast(pointer, c.POINTER(Version)).contents
        return {"major": value.major, "minor": value.minor}
    finally:
        free(error)
        free(pointer)


def inspect(display):
    xcb = c.CDLL("libxcb.so.1")
    present = c.CDLL("libxcb-present.so.0")
    dri3 = c.CDLL("libxcb-dri3.so.0")
    libc = c.CDLL(None)
    libc.free.argtypes = [c.c_void_p]
    libc.free.restype = None
    xcb.xcb_connect.argtypes = [c.c_char_p, c.POINTER(c.c_int)]
    xcb.xcb_connect.restype = c.c_void_p
    xcb.xcb_connection_has_error.argtypes = [c.c_void_p]
    xcb.xcb_connection_has_error.restype = c.c_int
    xcb.xcb_get_setup.argtypes = [c.c_void_p]
    xcb.xcb_get_setup.restype = c.c_void_p
    xcb.xcb_setup_roots_iterator.argtypes = [c.c_void_p]
    xcb.xcb_setup_roots_iterator.restype = ScreenIterator
    xcb.xcb_screen_next.argtypes = [c.POINTER(ScreenIterator)]
    xcb.xcb_screen_next.restype = None
    xcb.xcb_disconnect.argtypes = [c.c_void_p]
    xcb.xcb_disconnect.restype = None
    screen = c.c_int()
    connection = xcb.xcb_connect(display.encode() if display else None, c.byref(screen))
    if not connection:
        raise RuntimeError("XCB did not return a connection")
    try:
        error = xcb.xcb_connection_has_error(connection)
        if error:
            raise RuntimeError(f"XCB connection failed with code {error}")
        versions = {
            "present": query_version(connection, present, "xcb_present", libc.free),
            "dri3": query_version(connection, dri3, "xcb_dri3", libc.free),
        }
        iterator = xcb.xcb_setup_roots_iterator(xcb.xcb_get_setup(connection))
        for _ in range(screen.value):
            xcb.xcb_screen_next(c.byref(iterator))
        if iterator.remaining <= 0 or not iterator.data:
            raise RuntimeError("requested X screen is unavailable")
        root = c.cast(iterator.data, c.POINTER(c.c_uint32)).contents.value
        present.xcb_present_query_capabilities.argtypes = [c.c_void_p, c.c_uint32]
        present.xcb_present_query_capabilities.restype = Cookie
        present.xcb_present_query_capabilities_reply.argtypes = [
            c.c_void_p, Cookie, c.POINTER(c.c_void_p)
        ]
        present.xcb_present_query_capabilities_reply.restype = c.c_void_p
        error = c.c_void_p()
        pointer = present.xcb_present_query_capabilities_reply(
            connection, present.xcb_present_query_capabilities(connection, root), c.byref(error)
        )
        try:
            if error.value or not pointer:
                raise RuntimeError("Present root capabilities query failed")
            capabilities = c.cast(pointer + 8, c.POINTER(c.c_uint32)).contents.value
        finally:
            libc.free(error)
            libc.free(pointer)
        names = {1: "ASYNC", 2: "FENCE", 4: "UST", 8: "ASYNC_MAY_TEAR", 16: "SYNCOBJ"}
        return {
            "schema_version": 1,
            "qualification": "NOT_EVALUATED",
            "display": display or os.environ.get("DISPLAY"),
            "session_type_environment_only": os.environ.get("XDG_SESSION_TYPE"),
            "screen": screen.value,
            "root_window": root,
            "versions": versions,
            "root_present_capabilities": capabilities,
            "capability_names": [name for bit, name in names.items() if capabilities & bit],
            "unknown_capability_bits": capabilities & ~sum(names),
            "presentations_observed": 0,
        }
    finally:
        xcb.xcb_disconnect(connection)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Query XCB Present/DRI3 without creating a window.")
    parser.add_argument("--display", help="Existing X server; defaults to DISPLAY.")
    args = parser.parse_args()
    try:
        print(json.dumps(inspect(args.display), indent=2))
    except (OSError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
