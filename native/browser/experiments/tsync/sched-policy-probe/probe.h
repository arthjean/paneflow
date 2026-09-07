#ifndef PANEFLOW_SCHED_POLICY_PROBE_H_
#define PANEFLOW_SCHED_POLICY_PROBE_H_

#include <pthread.h>
#include <stdint.h>
#include <sys/types.h>

#include <atomic>
#include <string>

struct Operation {
  std::string target;
  std::string method;
  uint64_t scheduling_policy = 0;
  pid_t process_pid = 0;
  pid_t external_pid = 0;
  pid_t worker_tid = 0;
  pthread_t worker_handle{};
  bool invalid_pointer = false;
  bool check_no_new_privileges = true;
};

struct Worker {
  std::atomic<pid_t> tid{0};
  std::atomic<bool> go{false};
  std::atomic<bool> done{false};
  Operation* operation = nullptr;
  int scheduler_before = -1;
  int scheduler_after = -1;
  int priority_before = -1;
  int priority_after = -1;
  int no_new_privileges = -1;
};

[[noreturn]] void Fail(const char* operation);
void Require(bool condition, const char* operation);
void Perform(const Operation& operation);
void* RunWorker(void* argument);
void StartWorker(Worker& worker, pthread_t& handle);
void FinishWorker(Worker& worker, pthread_t handle);

#endif
