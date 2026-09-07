#include "probe.h"

#include <errno.h>
#include <sched.h>
#include <stdio.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>

#include "sandbox/linux/services/syscall_wrappers.h"
#include "sandbox/linux/system_headers/linux_syscalls.h"

[[noreturn]] void Fail(const char* operation) {
  fprintf(stderr, "probe assertion failed: %s (errno=%d)\n", operation, errno);
  _exit(70);
}

void Require(bool condition, const char* operation) {
  if (!condition) {
    Fail(operation);
  }
}

void Perform(const Operation& operation) {
  pid_t target = 0;
  if (operation.target == "pid") {
    target = operation.process_pid;
  } else if (operation.target == "self") {
    target = sandbox::sys_gettid();
  } else if (operation.target == "worker") {
    target = operation.worker_tid;
  } else if (operation.target == "external") {
    target = operation.external_pid;
  } else {
    Require(operation.target == "zero", "unknown target");
  }
  int number = __NR_sched_setscheduler;
  if (operation.method == "get_scheduler") {
    number = __NR_sched_getscheduler;
  } else if (operation.method == "get_param") {
    number = __NR_sched_getparam;
  } else if (operation.method == "set_param") {
    number = __NR_sched_setparam;
  } else if (operation.method == "get_affinity") {
    number = __NR_sched_getaffinity;
  } else if (operation.method == "set_affinity") {
    number = __NR_sched_setaffinity;
  } else {
    Require(operation.method == "raw" || operation.method == "pthread",
            "unknown method");
  }
  sched_param parameter{};
  auto* pointer = operation.invalid_pointer
                      ? reinterpret_cast<sched_param*>(uintptr_t{1})
                      : &parameter;
  cpu_set_t affinity;
  CPU_ZERO(&affinity);
  CPU_SET(0, &affinity);
  if (operation.check_no_new_privileges) {
    Require(prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1,
            "calling thread lacks no_new_privs");
  }
  printf("{\"event\":\"armed\",\"syscall_nr\":%d,\"target\":%d,"
         "\"caller_tid\":%d,\"scheduling_policy\":%llu}\n",
         number, target, sandbox::sys_gettid(),
         static_cast<unsigned long long>(operation.scheduling_policy));
  errno = 0;
  long result;
  if (operation.method == "pthread") {
    Require(operation.target == "worker" && !operation.invalid_pointer,
            "pthread target requires a valid worker");
    result = pthread_setschedparam(operation.worker_handle,
                                  static_cast<int>(operation.scheduling_policy),
                                  pointer);
  } else if (number == __NR_sched_setscheduler) {
    result = syscall(number, target, operation.scheduling_policy, pointer);
  } else if (number == __NR_sched_getscheduler) {
    result = syscall(number, target);
  } else if (number == __NR_sched_getparam || number == __NR_sched_setparam) {
    result = syscall(number, target, pointer);
  } else {
    result = syscall(number, target, sizeof(affinity), &affinity);
  }
  const int error = errno;
  printf("{\"event\":\"result\",\"result\":%ld,\"errno\":%d}\n",
         result, error);
  if (result == 0 && number == __NR_sched_setscheduler &&
      operation.method == "raw") {
    Require(target == 0 || target == operation.process_pid ||
                target == sandbox::sys_gettid(),
            "foreign scheduler operation unexpectedly succeeded");
    Require(sched_getscheduler(target) == SCHED_BATCH,
            "allowed scheduling operation did not reach kernel");
    Require(sched_setscheduler(target, SCHED_OTHER, &parameter) == 0,
            "restore own scheduler");
  }
}

void* RunWorker(void* argument) {
  auto& worker = *static_cast<Worker*>(argument);
  sched_param parameter{};
  worker.scheduler_before = sched_getscheduler(0);
  Require(sched_getparam(0, &parameter) == 0, "read worker priority before");
  worker.priority_before = parameter.sched_priority;
  worker.tid.store(sandbox::sys_gettid());
  while (!worker.go.load()) {
    usleep(1000);
  }
  worker.no_new_privileges = prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0);
  if (worker.operation) {
    Perform(*worker.operation);
  }
  worker.scheduler_after = sched_getscheduler(0);
  Require(sched_getparam(0, &parameter) == 0, "read worker priority after");
  worker.priority_after = parameter.sched_priority;
  worker.done.store(true);
  return nullptr;
}

void StartWorker(Worker& worker, pthread_t& handle) {
  Require(pthread_create(&handle, nullptr, RunWorker, &worker) == 0,
          "create worker");
  while (worker.tid.load() == 0) {
    usleep(1000);
  }
  Require(worker.scheduler_before == SCHED_OTHER && worker.priority_before == 0,
          "worker scheduler precondition");
}

void FinishWorker(Worker& worker, pthread_t handle) {
  worker.go.store(true);
  Require(pthread_join(handle, nullptr) == 0, "join worker");
  Require(worker.done.load() && worker.no_new_privileges == 1,
          "worker did not inherit sandbox state");
  Require(worker.scheduler_before == worker.scheduler_after &&
              worker.priority_before == worker.priority_after,
          "worker scheduling state changed");
}
