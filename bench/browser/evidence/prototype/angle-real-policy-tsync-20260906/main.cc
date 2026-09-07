#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <EGL/eglext_angle.h>
#include <GLES2/gl2.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <atomic>
#include <cstring>
#include <string>

#include "base/at_exit.h"
#include "base/command_line.h"
#include "base/feature_list.h"
#include "base/functional/bind.h"
#include "content/common/gpu_pre_sandbox_hook_linux.h"
#include "sandbox/linux/syscall_broker/broker_file_permission.h"
#include "sandbox/policy/linux/sandbox_linux.h"
#include "sandbox/policy/mojom/sandbox.mojom.h"

namespace {

[[noreturn]] void Fail(const char* operation) {
  fprintf(stderr, "{\"event\":\"failure\",\"operation\":\"%s\",\"errno\":%d}\n",
          operation, errno);
  exit(1);
}

template <typename T>
T Load(void* library, const char* name) {
  auto symbol = reinterpret_cast<T>(dlsym(library, name));
  if (!symbol) {
    Fail(name);
  }
  return symbol;
}

#define EGL_FUNCTIONS(X)     \
  X(eglGetProcAddress)        \
  X(eglInitialize)            \
  X(eglBindAPI)               \
  X(eglChooseConfig)          \
  X(eglCreateContext)         \
  X(eglCreatePbufferSurface)  \
  X(eglMakeCurrent)           \
  X(eglDestroySurface)        \
  X(eglDestroyContext)        \
  X(eglTerminate)             \
  X(eglReleaseThread)

#define GLES_FUNCTIONS(X)    \
  X(glGetString)             \
  X(glViewport)              \
  X(glCreateShader)          \
  X(glShaderSource)          \
  X(glCompileShader)         \
  X(glGetShaderiv)           \
  X(glCreateProgram)         \
  X(glAttachShader)          \
  X(glBindAttribLocation)    \
  X(glLinkProgram)           \
  X(glGetProgramiv)          \
  X(glUseProgram)            \
  X(glVertexAttribPointer)   \
  X(glEnableVertexAttribArray) \
  X(glDrawArrays)            \
  X(glFinish)                \
  X(glReadPixels)            \
  X(glGetError)              \
  X(glDeleteProgram)         \
  X(glDeleteShader)

struct NativeGl {
#define DECLARE_FUNCTION(name) decltype(&::name) name = nullptr;
  EGL_FUNCTIONS(DECLARE_FUNCTION)
  GLES_FUNCTIONS(DECLARE_FUNCTION)
#undef DECLARE_FUNCTION

  void Initialize(const std::string& root) {
    void* egl = dlopen((root + "/libEGL.so").c_str(), RTLD_NOW | RTLD_LOCAL);
    void* gles = dlopen((root + "/libGLESv2.so").c_str(), RTLD_NOW | RTLD_LOCAL);
    if (!egl || !gles) {
      Fail("dlopen native EGL/GLES");
    }
#define LOAD_EGL(name) name = Load<decltype(name)>(egl, #name);
    EGL_FUNCTIONS(LOAD_EGL)
#undef LOAD_EGL
#define LOAD_GLES(name) name = Load<decltype(name)>(gles, #name);
    GLES_FUNCTIONS(LOAD_GLES)
#undef LOAD_GLES
  }

  GLuint Shader(GLenum kind, const char* source) {
    GLuint id = glCreateShader(kind);
    glShaderSource(id, 1, &source, nullptr);
    glCompileShader(id);
    GLint ok = 0;
    glGetShaderiv(id, GL_COMPILE_STATUS, &ok);
    if (!ok) {
      Fail("shader compile");
    }
    return id;
  }

  void Draw(unsigned frame) {
    char fragment[256];
    snprintf(fragment, sizeof(fragment),
             "precision mediump float; void main(){gl_FragColor="
             "vec4(%.8f,0.25,0.75,1.0);}",
             (frame % 31 + 1) / 32.0);
    GLuint vs = Shader(GL_VERTEX_SHADER,
                       "attribute vec2 p; void main(){"
                       "gl_Position=vec4(p,0.0,1.0);}");
    GLuint fs = Shader(GL_FRAGMENT_SHADER, fragment);
    GLuint program = glCreateProgram();
    glAttachShader(program, vs);
    glAttachShader(program, fs);
    glBindAttribLocation(program, 0, "p");
    glLinkProgram(program);
    GLint ok = 0;
    glGetProgramiv(program, GL_LINK_STATUS, &ok);
    if (!ok) {
      Fail("program link");
    }
    glUseProgram(program);
    const GLfloat triangle[] = {-1, -1, 3, -1, -1, 3};
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, triangle);
    glEnableVertexAttribArray(0);
    glDrawArrays(GL_TRIANGLES, 0, 3);
    glFinish();
    unsigned char pixel[4] = {};
    glReadPixels(16, 16, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, pixel);
    int expected = static_cast<int>((frame % 31 + 1) * 255.0 / 32.0 + 0.5);
    if (abs(pixel[0] - expected) > 2 || abs(pixel[1] - 64) > 2 ||
        abs(pixel[2] - 191) > 2 || pixel[3] != 255 ||
        glGetError() != GL_NO_ERROR) {
      Fail("pixel verification");
    }
    glUseProgram(0);
    glDeleteProgram(program);
    glDeleteShader(vs);
    glDeleteShader(fs);
  }
};

struct AccessResult {
  int result;
  int error;
};

AccessResult TryOpen(const char* path) {
  errno = 0;
  int fd = open(path, O_RDONLY | O_CLOEXEC);
  AccessResult result{fd >= 0 ? 0 : -1, errno};
  if (fd >= 0) {
    close(fd);
  }
  return result;
}

std::string ChooseAllowedFile(
    const sandbox::policy::SandboxLinux::Options& options) {
  const auto permissions = content::FilePermissionsForGpu(options);
  const char* candidates[] = {
      "/etc/drirc", "/usr/share/vulkan/icd.d/intel_icd.x86_64.json",
      "/usr/share/vulkan/icd.d/radeon_icd.x86_64.json",
      "/usr/share/vulkan/icd.d/nvidia_icd.json"};
  for (const char* path : candidates) {
    bool permitted = false;
    for (const auto& permission : permissions) {
      auto [matched, temporary] = permission.CheckOpen(path, O_RDONLY);
      permitted |= matched != nullptr && !temporary;
    }
    if (!permitted) {
      continue;
    }
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
      continue;
    }
    struct stat metadata = {};
    bool regular = fstat(fd, &metadata) == 0 && S_ISREG(metadata.st_mode);
    close(fd);
    if (regular) {
      return path;
    }
  }
  Fail("no readable regular file in Chromium GPU broker permissions");
}

struct Witness {
  std::atomic<bool> ready{false};
  std::atomic<bool> go{false};
  const char* allowed_path = nullptr;
  AccessResult before{};
  AccessResult denied{};
  AccessResult allowed{};
  int seccomp = -1;
};

void* RunWitness(void* argument) {
  auto& witness = *static_cast<Witness*>(argument);
  witness.before = TryOpen("/etc/passwd");
  witness.ready.store(true);
  while (!witness.go.load()) {
    usleep(1000);
  }
  witness.denied = TryOpen("/etc/passwd");
  witness.allowed = TryOpen(witness.allowed_path);
  witness.seccomp = prctl(PR_GET_SECCOMP);
  return nullptr;
}

void CheckAccess(const char* thread,
                 AccessResult denied,
                 AccessResult allowed,
                 int seccomp) {
  printf("{\"event\":\"access\",\"thread\":\"%s\",\"denied_result\":%d,"
         "\"denied_errno\":%d,\"allowed_result\":%d,\"allowed_errno\":%d,"
         "\"seccomp\":%d}\n",
         thread, denied.result, denied.error, allowed.result, allowed.error,
         seccomp);
  if (denied.result != -1 || denied.error != EACCES || allowed.result != 0 ||
      seccomp != 2) {
    Fail("real GPU policy access assertions");
  }
}

}

int main(int argc, char** argv) {
  setvbuf(stdout, nullptr, _IOLBF, 0);
  base::AtExitManager at_exit;
  base::CommandLine::Init(argc, argv);
  auto* command_line = base::CommandLine::ForCurrentProcess();
  std::string angle_root = command_line->GetSwitchValueASCII("angle-root");
  if (angle_root.empty()) {
    fprintf(stderr, "usage: angle_probe --angle-root=/absolute/cef/Release "
                    "[--nvidia]\n");
    return 2;
  }
  command_line->AppendSwitchASCII("type", "gpu-process");
  base::FeatureList::InitInstance("", "");
  sandbox::policy::SandboxLinux::Options options;
  options.allow_threads_during_sandbox_init = true;
  options.use_nvidia_specific_policies = command_line->HasSwitch("nvidia");
  std::string allowed_path = ChooseAllowedFile(options);
  if (TryOpen("/etc/passwd").result != 0) {
    Fail("denied file must be readable before sandbox");
  }
  auto* linux_sandbox = sandbox::policy::SandboxLinux::GetInstance();
  content::PrepareGpuSandboxBroker(options);
  printf("{\"event\":\"broker_prepared\",\"allowed_path\":\"%s\","
         "\"single_threaded\":%s}\n",
         allowed_path.c_str(), linux_sandbox->IsSingleThreaded() ? "true" : "false");

  NativeGl gl;
  gl.Initialize(angle_root);
  auto platform_display = reinterpret_cast<PFNEGLGETPLATFORMDISPLAYEXTPROC>(
      gl.eglGetProcAddress("eglGetPlatformDisplayEXT"));
  if (!platform_display) {
    Fail("ANGLE platform extension");
  }
  const EGLint angle_attributes[] = {
      EGL_PLATFORM_ANGLE_TYPE_ANGLE, EGL_PLATFORM_ANGLE_TYPE_VULKAN_ANGLE,
      EGL_PLATFORM_ANGLE_DEVICE_TYPE_ANGLE,
      EGL_PLATFORM_ANGLE_DEVICE_TYPE_HARDWARE_ANGLE, EGL_NONE};
  EGLDisplay display = platform_display(EGL_PLATFORM_ANGLE_ANGLE,
                                       nullptr, angle_attributes);
  EGLint count = 0;
  if (display == EGL_NO_DISPLAY || !gl.eglInitialize(display, nullptr, nullptr) ||
      !gl.eglBindAPI(EGL_OPENGL_ES_API)) {
    Fail("EGL initialize selected render node");
  }
  const EGLint attributes[] = {
      EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
      EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8,
      EGL_NONE};
  EGLConfig config;
  if (!gl.eglChooseConfig(display, attributes, &config, 1, &count) || count != 1) {
    Fail("choose config");
  }
  const EGLint context_attributes[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
  EGLContext context =
      gl.eglCreateContext(display, config, EGL_NO_CONTEXT, context_attributes);
  const EGLint surface_attributes[] = {EGL_WIDTH, 32, EGL_HEIGHT, 32, EGL_NONE};
  EGLSurface surface = gl.eglCreatePbufferSurface(display, config, surface_attributes);
  if (context == EGL_NO_CONTEXT || surface == EGL_NO_SURFACE ||
      !gl.eglMakeCurrent(display, surface, surface, context)) {
    Fail("make current");
  }
  printf("{\"event\":\"renderer\",\"renderer\":\"%s\",\"version\":\"%s\"}\n",
         gl.glGetString(GL_RENDERER), gl.glGetString(GL_VERSION));
  gl.glViewport(0, 0, 32, 32);
  gl.Draw(0);
  Witness witness;
  witness.allowed_path = allowed_path.c_str();
  pthread_t worker;
  if (pthread_create(&worker, nullptr, RunWitness, &witness)) {
    Fail("pthread_create");
  }
  while (!witness.ready.load()) {
    usleep(1000);
  }
  if (witness.before.result != 0 || linux_sandbox->IsSingleThreaded()) {
    Fail("witness preconditions");
  }
  puts("{\"event\":\"sandbox_initializing\",\"policy\":\"Chromium kGpu\","
       "\"allow_threads_during_sandbox_init\":true,\"namespace_layer\":false}");
  if (!linux_sandbox->InitializeSandbox(
          sandbox::mojom::Sandbox::kGpu,
          base::BindOnce(&content::GpuPreSandboxHookWithPreparedBroker), options) ||
      !linux_sandbox->seccomp_bpf_started()) {
    Fail("InitializeSandbox real GPU TSYNC");
  }
  puts("{\"event\":\"sandbox_started\",\"seccomp_bpf_started\":true}");
  witness.go.store(true);
  CheckAccess("main", TryOpen("/etc/passwd"), TryOpen(allowed_path.c_str()),
              prctl(PR_GET_SECCOMP));
  if (pthread_join(worker, nullptr)) {
    Fail("pthread_join");
  }
  CheckAccess("witness", witness.denied, witness.allowed, witness.seccomp);
  for (unsigned frame = 1; frame <= 120; ++frame) {
    gl.Draw(frame);
  }
  puts("{\"event\":\"render_verified\",\"frames_after_filter\":120}");
  if (!gl.eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT) ||
      !gl.eglDestroySurface(display, surface) ||
      !gl.eglDestroyContext(display, context) || !gl.eglTerminate(display) ||
      !gl.eglReleaseThread()) {
    Fail("EGL shutdown");
  }
  puts("{\"event\":\"completed\",\"verified_frames_after_filter\":120,"
       "\"egl_shutdown\":true,\"full_cef_qualification\":false}");
  return 0;
}
