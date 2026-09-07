#!/usr/bin/env python3
import ctypes as c
import json
import sys

egl = c.CDLL('libEGL.so.1')
egl.eglGetProcAddress.restype = c.c_void_p
egl.eglGetProcAddress.argtypes = [c.c_char_p]
def proc(name, result, *args):
    address = egl.eglGetProcAddress(name.encode())
    if not address:
        raise RuntimeError(name + ' unavailable')
    return c.CFUNCTYPE(result, *args)(address)
query_devices = proc('eglQueryDevicesEXT', c.c_uint, c.c_int, c.POINTER(c.c_void_p), c.POINTER(c.c_int))
query_string = proc('eglQueryDeviceStringEXT', c.c_char_p, c.c_void_p, c.c_int)
get_display = proc('eglGetPlatformDisplayEXT', c.c_void_p, c.c_int, c.c_void_p, c.c_void_p)
initialize = proc('eglInitialize', c.c_uint, c.c_void_p, c.c_void_p, c.c_void_p)
query_formats = proc('eglQueryDmaBufFormatsEXT', c.c_uint, c.c_void_p, c.c_int, c.POINTER(c.c_int), c.POINTER(c.c_int))
query_modifiers = proc('eglQueryDmaBufModifiersEXT', c.c_uint, c.c_void_p, c.c_int, c.c_int, c.POINTER(c.c_uint64), c.POINTER(c.c_uint), c.POINTER(c.c_int))
terminate = proc('eglTerminate', c.c_uint, c.c_void_p)
devices = (c.c_void_p * 16)()
count = c.c_int()
if not query_devices(16, devices, c.byref(count)):
    raise RuntimeError('device query failed')
for device in devices[:count.value]:
    node = query_string(device, 0x3377)
    if node != sys.argv[1].encode():
        continue
    display = get_display(0x313F, device, None)
    if not initialize(display, None, None):
        raise RuntimeError('initialize failed')
    try:
        formats = (c.c_int * 1024)()
        if not query_formats(display, len(formats), formats, c.byref(count)):
            raise RuntimeError('format query failed')
        for fmt in formats[:count.value]:
            name = fmt.to_bytes(4, 'little').decode('ascii', errors='replace')
            if name not in ['AR24', 'AB24', 'XR24', 'XB24']:
                continue
            modifiers = (c.c_uint64 * 1024)()
            external = (c.c_uint * 1024)()
            if not query_modifiers(display, fmt, len(modifiers), modifiers, external, c.byref(count)):
                raise RuntimeError('modifier query failed')
            print(json.dumps({'render_node': sys.argv[1], 'fourcc': name, 'modifiers': [{'value': value, 'hex': hex(value), 'external_only': bool(external[index])} for index, value in enumerate(modifiers[:count.value])]}))
    finally:
        terminate(display)
    break
else:
    raise RuntimeError('render node absent')
