#include "probe.h"

#include <errno.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <unistd.h>

#include <memory>
#include <utility>

#include "base/at_exit.h"
#include "base/command_line.h"
#include "base/feature_list.h"
#include "content/common/gpu_pre_sandbox_hook_linux.h"
#include "media/media_buildflags.h"
#include "sandbox/linux/seccomp-bpf/sandbox_bpf.h"
#include "sandbox/policy/linux/bpf_base_policy_linux.h"
#include "sandbox/policy/linux/bpf_gpu_policy_linux.h"
#include "sandbox/policy/linux/sandbox_linux.h"
#include "sandbox/policy/linux/sandbox_seccomp_bpf_linux.h"
#include "sandbox/policy/mojom/sandbox.mojom.h"

int main(int argc, char** argv) {
  setvbuf(stdout, nullptr, _IOLBF, 0);
  Require(getuid() != 0 && getuid() == geteuid(), "ordinary user required");
  rlimit core_limit{0, 0};
  Require(setrlimit(RLIMIT_CORE, &core_limit) == 0, "disable core files");
  Require(sched_getscheduler(0) == SCHED_OTHER, "main scheduler precondition");
  base::AtExitManager at_exit;
  base::CommandLine::Init(argc, argv);
  base::FeatureList::InitInstance("", "");
  auto* command = base::CommandLine::ForCurrentProcess();
  auto value = [command](const char* name) {
    return command->GetSwitchValueASCII(name);
  };
  Operation operation;
  operation.target = value("target");
  operation.method = value("method");
  operation.process_pid = getpid();
  operation.external_pid = getppid();
  operation.invalid_pointer = command->HasSwitch("invalid-pointer");
  const std::string policy_text = value("scheduling-policy");
  char* end = nullptr;
  errno = 0;
  operation.scheduling_policy = strtoull(policy_text.c_str(), &end, 0);
  Require(errno == 0 && !policy_text.empty() && end && *end == '\0',
          "scheduling-policy must be an unsigned scalar");
  auto sandbox_type = sandbox::mojom::Sandbox::kGpu;
  const bool default_gpu_policy = value("sandbox") == "gpu-default";
  if (value("sandbox") == "renderer") {
    sandbox_type = sandbox::mojom::Sandbox::kRenderer;
    operation.check_no_new_privileges = false;
  } else if (value("sandbox") == "model") {
    sandbox_type = sandbox::mojom::Sandbox::kOnDeviceModelExecution;
  } else if (value("sandbox") == "video-encoder") {
#if BUILDFLAG(USE_LINUX_VIDEO_ACCELERATION)
    sandbox_type = sandbox::mojom::Sandbox::kHardwareVideoEncoding;
#else
    puts("{\"event\":\"factory_unavailable\","
         "\"factory\":\"kHardwareVideoEncoding\","
         "\"USE_LINUX_VIDEO_ACCELERATION\":false}");
    _exit(77);
#endif
  } else {
    Require(value("sandbox") == "gpu" || default_gpu_policy,
            "unknown sandbox policy");
  }
  const std::string caller = value("caller");
  Require(caller == "main" || caller == "before" || caller == "after",
          "unknown caller thread");
  Require(operation.check_no_new_privileges || caller == "main",
          "renderer fatal control requires the main caller");
  command->AppendSwitchASCII(
      "type", sandbox_type == sandbox::mojom::Sandbox::kRenderer ? "renderer"
                                                               : "gpu-process");
  sandbox::policy::SandboxLinux::Options options;
  if (sandbox_type != sandbox::mojom::Sandbox::kRenderer) {
    content::PrepareGpuSandboxBroker(options);
  }
  Worker target_worker;
  pthread_t target_handle{};
  StartWorker(target_worker, target_handle);
  operation.worker_tid = target_worker.tid.load();
  operation.worker_handle = target_handle;
  sched_param parameter{};
  Require(sched_setscheduler(operation.worker_tid, SCHED_BATCH, &parameter) == 0,
          "foreign worker scheduling must work before sandbox");
  Require(sched_getscheduler(operation.worker_tid) == SCHED_BATCH,
          "pre-sandbox kernel scheduling control");
  Require(sched_setscheduler(operation.worker_tid, SCHED_OTHER, &parameter) == 0,
          "restore pre-sandbox target worker");
  Worker calling_worker;
  pthread_t caller_handle{};
  calling_worker.operation = &operation;
  if (caller == "before") {
    StartWorker(calling_worker, caller_handle);
  }
  std::unique_ptr<sandbox::policy::BPFBasePolicy> policy;
  if (default_gpu_policy) {
    policy = std::make_unique<sandbox::policy::GpuProcessPolicy>(
        sandbox::policy::MremapPolicy::kBlock);
  } else {
    policy = sandbox::policy::SandboxSeccompBPF::PolicyForSandboxType(
        sandbox_type, options);
  }
  sandbox::SandboxBPF filter(std::move(policy));
  Require(filter.StartSandbox(sandbox::SandboxBPF::SeccompLevel::MULTI_THREADED),
          "install actual Chromium policy with TSYNC");
  if (operation.check_no_new_privileges) {
    Require(prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1,
            "main no_new_privs after TSYNC");
  }
  printf("{\"event\":\"sandbox_started\",\"tsync\":true,"
         "\"policy_source\":\"%s\",\"namespace_layer\":false,"
         "\"no_new_privs_probe\":\"%s\"}\n",
         default_gpu_policy ? "Chromium default GpuProcessPolicy"
                            : "Chromium PolicyForSandboxType",
         operation.check_no_new_privileges ? "checked"
                                          : "forbidden_by_renderer_policy");
  if (caller == "after") {
    StartWorker(calling_worker, caller_handle);
  }
  if (caller == "main") {
    Perform(operation);
  } else {
    FinishWorker(calling_worker, caller_handle);
  }
  Require(operation.check_no_new_privileges,
          "renderer scheduling denial unexpectedly returned");
  FinishWorker(target_worker, target_handle);
  printf("{\"event\":\"completed\",\"target_tid\":%d,"
         "\"target_scheduler_before\":%d,\"target_scheduler_after\":%d,"
         "\"target_priority_before\":%d,\"target_priority_after\":%d,"
         "\"target_no_new_privs\":%d}\n",
         operation.worker_tid, target_worker.scheduler_before,
         target_worker.scheduler_after, target_worker.priority_before,
         target_worker.priority_after, target_worker.no_new_privileges);
  _exit(0);
}
