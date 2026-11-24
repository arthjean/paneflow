#include <stdlib.h>
#include <unistd.h>
#include <sys/wait.h>
int main(int argc, char **argv) {
  const char *script = getenv("FAKE_AGENT_SCRIPT");
  if (!script) script = "/Users/m1/pfq-cli-fake/fake-agent.sh";
  pid_t p = fork();
  if (p == 0) { execl("/bin/bash", "bash", script, (char *)0); _exit(127); }
  int st = 0; waitpid(p, &st, 0);
  return WIFEXITED(st) ? WEXITSTATUS(st) : 1;
}
