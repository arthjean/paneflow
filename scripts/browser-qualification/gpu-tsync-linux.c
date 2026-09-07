#define _GNU_SOURCE
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <dirent.h>
#include <errno.h>
#include <pthread.h>
#include <seccomp.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/socket.h>
#include <unistd.h>

static atomic_int witness_ready, witness_go;
static long witness_before, witness_after;
static int witness_errno;

static void fail(const char *operation) {
    fprintf(stderr, "%s failed (errno=%d egl=0x%x gl=0x%x)\n", operation, errno, eglGetError(), glGetError());
    exit(1);
}

static void *witness(void *unused) {
    (void)unused;
    pthread_setname_np(pthread_self(), "tsync-witness");
    witness_before = syscall(SYS_getppid);
    atomic_store(&witness_ready, 1);
    while (!atomic_load(&witness_go)) usleep(1000);
    errno = 0;
    witness_after = syscall(SYS_getppid);
    witness_errno = errno;
    return NULL;
}

static void snapshot(const char *phase) {
    DIR *directory = opendir("/proc/self/task");
    if (!directory) fail("opendir");
    struct dirent *entry;
    while ((entry = readdir(directory))) {
        if (entry->d_name[0] == '.') continue;
        char path[512], line[512], name[64] = "unknown";
        snprintf(path, sizeof(path), "/proc/self/task/%s/status", entry->d_name);
        FILE *file = fopen(path, "r");
        if (!file) continue;
        int seccomp = -1, filters = -1, nnp = -1;
        while (fgets(line, sizeof(line), file)) {
            if (!strncmp(line, "Name:", 5)) sscanf(line, "Name: %63s", name);
            if (!strncmp(line, "Seccomp:", 8)) sscanf(line, "Seccomp: %d", &seccomp);
            if (!strncmp(line, "Seccomp_filters:", 16)) sscanf(line, "Seccomp_filters: %d", &filters);
            if (!strncmp(line, "NoNewPrivs:", 11)) sscanf(line, "NoNewPrivs: %d", &nnp);
        }
        fclose(file);
        for (char *p = name; *p; ++p) if (*p == '"' || *p == '\\') *p = '_';
        printf("{\"event\":\"thread\",\"phase\":\"%s\",\"tid\":%s,\"name\":\"%s\",\"seccomp\":%d,\"filters\":%d,\"no_new_privs\":%d}\n", phase, entry->d_name, name, seccomp, filters, nnp);
    }
    closedir(directory);
}

static GLuint shader(GLenum kind, const char *source) {
    GLuint id = glCreateShader(kind);
    glShaderSource(id, 1, &source, NULL);
    glCompileShader(id);
    GLint ok = 0;
    glGetShaderiv(id, GL_COMPILE_STATUS, &ok);
    if (!ok) fail("shader compile");
    return id;
}

static void draw(unsigned frame) {
    char fragment[256];
    snprintf(fragment, sizeof(fragment), "precision mediump float; void main(){gl_FragColor=vec4(%.8f,0.25,0.75,1.0);}", (frame % 31 + 1) / 32.0);
    GLuint vs = shader(GL_VERTEX_SHADER, "attribute vec2 p; void main(){gl_Position=vec4(p,0.0,1.0);}");
    GLuint fs = shader(GL_FRAGMENT_SHADER, fragment);
    GLuint program = glCreateProgram();
    glAttachShader(program, vs);
    glAttachShader(program, fs);
    glBindAttribLocation(program, 0, "p");
    glLinkProgram(program);
    GLint ok = 0;
    glGetProgramiv(program, GL_LINK_STATUS, &ok);
    if (!ok) fail("program link");
    glUseProgram(program);
    const GLfloat triangle[] = {-1,-1,3,-1,-1,3};
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, triangle);
    glEnableVertexAttribArray(0);
    glDrawArrays(GL_TRIANGLES, 0, 3);
    glFinish();
    unsigned char pixel[4];
    glReadPixels(16, 16, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, pixel);
    int expected = (int)((frame % 31 + 1) * 255.0 / 32.0 + 0.5);
    if (abs(pixel[0]-expected)>2 || abs(pixel[1]-64)>2 || abs(pixel[2]-191)>2 || pixel[3]!=255 || glGetError()!=GL_NO_ERROR) fail("pixel verification");
    glUseProgram(0);
    glDeleteProgram(program);
    glDeleteShader(vs);
    glDeleteShader(fs);
}

static void install_filter(int tsync, int bounded) {
    scmp_filter_ctx filter = seccomp_init(bounded ? SCMP_ACT_TRAP : SCMP_ACT_ALLOW);
    if (!filter) fail("seccomp_init");
    if (bounded) {
        const char *allowed[] = {
            "read", "write", "readv", "writev", "close", "close_range", "lseek", "pread64", "pwrite64",
            "openat", "newfstatat", "fstat", "statx", "getdents64", "readlink", "readlinkat", "access", "faccessat", "faccessat2",
            "mmap", "mprotect", "munmap", "mremap", "madvise", "brk", "futex", "futex_waitv", "futex_wait", "futex_wake",
            "rt_sigaction", "rt_sigprocmask", "rt_sigreturn", "sigaltstack", "restart_syscall", "tgkill",
            "clone", "clone3", "set_robust_list", "rseq", "set_tid_address", "getpid", "gettid", "getuid", "geteuid",
            "getgid", "getegid", "sched_yield", "sched_getaffinity", "sched_setaffinity", "sched_getparam", "sched_getscheduler",
            "clock_gettime", "clock_nanosleep", "nanosleep", "gettimeofday", "getrusage", "getrandom",
            "ioctl", "fcntl", "dup", "dup2", "dup3", "poll", "ppoll", "pselect6", "eventfd2", "epoll_create1", "epoll_ctl", "epoll_wait", "epoll_pwait",
            "prctl", "prlimit64", "uname", "exit", "exit_group"
        };
        for (size_t i=0; i<sizeof(allowed)/sizeof(allowed[0]); ++i) {
            int nr = seccomp_syscall_resolve_name(allowed[i]);
            if (nr != __NR_SCMP_ERROR && seccomp_rule_add(filter, SCMP_ACT_ALLOW, nr, 0)) fail("allow rule");
        }
    }
    if (seccomp_rule_add(filter, SCMP_ACT_ERRNO(EPERM), SCMP_SYS(getppid), 0)) fail("marker rule");
    if (seccomp_attr_set(filter, SCMP_FLTATR_CTL_TSYNC, tsync)) fail("TSYNC attribute");
    int result = seccomp_load(filter);
    printf("{\"event\":\"filter_install\",\"tsync\":%d,\"bounded\":%d,\"result\":%d}\n", tsync, bounded, result);
    if (result) exit(2);
    seccomp_release(filter);
}

int main(int argc, char **argv) {
    if (argc != 4 && argc != 5) { fprintf(stderr, "usage: mesa-tsync RENDER_NODE TSYNC_0_OR_1 BOUNDED_0_OR_1 [trap-control]\n"); return 2; }
    if (argc == 5 && (strcmp(argv[4], "trap-control") || strcmp(argv[2], "1") || strcmp(argv[3], "1"))) return 2;
    setvbuf(stdout, NULL, _IOLBF, 0);
    PFNEGLQUERYDEVICESEXTPROC query = (PFNEGLQUERYDEVICESEXTPROC)eglGetProcAddress("eglQueryDevicesEXT");
    PFNEGLQUERYDEVICESTRINGEXTPROC query_string = (PFNEGLQUERYDEVICESTRINGEXTPROC)eglGetProcAddress("eglQueryDeviceStringEXT");
    PFNEGLGETPLATFORMDISPLAYEXTPROC platform_display = (PFNEGLGETPLATFORMDISPLAYEXTPROC)eglGetProcAddress("eglGetPlatformDisplayEXT");
    if (!query || !query_string || !platform_display) fail("EGL device extension");
    EGLDeviceEXT devices[16];
    EGLint count = 0;
    if (!query(16, devices, &count)) fail("query devices");
    EGLDisplay display = EGL_NO_DISPLAY;
    for (int i=0; i<count; ++i) {
        const char *node = query_string(devices[i], EGL_DRM_RENDER_NODE_FILE_EXT);
        if (node && !strcmp(node, argv[1])) display = platform_display(EGL_PLATFORM_DEVICE_EXT, devices[i], NULL);
    }
    if (display == EGL_NO_DISPLAY) fail("select render node");
    if (!eglInitialize(display, NULL, NULL) || !eglBindAPI(EGL_OPENGL_ES_API)) fail("EGL init");
    EGLint attributes[] = {EGL_SURFACE_TYPE,EGL_PBUFFER_BIT,EGL_RENDERABLE_TYPE,EGL_OPENGL_ES2_BIT,EGL_RED_SIZE,8,EGL_GREEN_SIZE,8,EGL_BLUE_SIZE,8,EGL_ALPHA_SIZE,8,EGL_NONE};
    EGLConfig config;
    if (!eglChooseConfig(display, attributes, &config, 1, &count) || count != 1) fail("choose config");
    EGLint context_attributes[] = {EGL_CONTEXT_CLIENT_VERSION,2,EGL_NONE};
    EGLContext context = eglCreateContext(display, config, EGL_NO_CONTEXT, context_attributes);
    EGLint surface_attributes[] = {EGL_WIDTH,32,EGL_HEIGHT,32,EGL_NONE};
    EGLSurface surface = eglCreatePbufferSurface(display, config, surface_attributes);
    if (context == EGL_NO_CONTEXT || surface == EGL_NO_SURFACE || !eglMakeCurrent(display,surface,surface,context)) fail("make current");
    printf("{\"event\":\"renderer\",\"renderer\":\"%s\",\"version\":\"%s\"}\n", glGetString(GL_RENDERER), glGetString(GL_VERSION));
    glViewport(0,0,32,32);
    draw(0);
    pthread_t thread;
    if (pthread_create(&thread, NULL, witness, NULL)) fail("pthread_create");
    while (!atomic_load(&witness_ready)) usleep(1000);
    snapshot("before");
    install_filter(atoi(argv[2]), atoi(argv[3]));
    snapshot("sealed");
    errno = 0;
    long main_marker = syscall(SYS_getppid);
    int main_errno = errno;
    atomic_store(&witness_go, 1);
    if (pthread_join(thread, NULL)) fail("pthread_join");
    printf("{\"event\":\"marker\",\"main_result\":%ld,\"main_errno\":%d,\"worker_before\":%ld,\"worker_result\":%ld,\"worker_errno\":%d}\n", main_marker,main_errno,witness_before,witness_after,witness_errno);
    if (main_marker!=-1 || main_errno!=EPERM || witness_before<=0 || (atoi(argv[2]) ? (witness_after!=-1 || witness_errno!=EPERM) : witness_after<=0)) fail("marker assertions");
    for (unsigned i=1; i<=120; ++i) draw(i);
    snapshot("after_render");
    eglMakeCurrent(display,EGL_NO_SURFACE,EGL_NO_SURFACE,EGL_NO_CONTEXT);
    eglDestroySurface(display,surface);
    eglDestroyContext(display,context);
    eglTerminate(display);
    if (argc == 5) {
        puts("{\"event\":\"trap_control\",\"denied_syscall\":\"socket\"}");
        syscall(SYS_socket, AF_INET, SOCK_STREAM, 0);
        fail("denied socket returned");
    }
    puts("{\"event\":\"completed\",\"verified_frames_after_filter\":120}");
    return 0;
}
