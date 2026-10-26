/* Lanceur natif de Hermes Map : exécute Resources/launch.zsh et attend la fin du serveur,
   pour que macOS attribue l'accès (Accès complet au disque) à Hermes Map.app. */
#include <limits.h>
#include <mach-o/dyld.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
extern char **environ;
int main(void) {
    char exe[PATH_MAX], real[PATH_MAX], script[PATH_MAX + 64];
    uint32_t n = sizeof exe;
    if (_NSGetExecutablePath(exe, &n) != 0 || !realpath(exe, real)) return 1;
    char *p = strrchr(real, '/'); if (!p) return 1; *p = 0;      /* .../Contents/MacOS */
    p = strrchr(real, '/'); if (!p) return 1; *p = 0;            /* .../Contents */
    snprintf(script, sizeof script, "%s/Resources/launch.zsh", real);
    char *argv[] = {"/bin/zsh", script, NULL};
    pid_t pid; int st = 0;
    if (posix_spawn(&pid, "/bin/zsh", NULL, NULL, argv, environ) != 0) return 1;
    while (waitpid(pid, &st, 0) < 0) {}
    return WIFEXITED(st) ? WEXITSTATUS(st) : 1;
}
