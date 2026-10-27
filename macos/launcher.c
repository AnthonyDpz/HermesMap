/* Lanceur natif de Hermes Map.
 *
 * - Exécute Resources/launch.zsh (qui démarre le serveur Python et ouvre la page), et reste en vie
 *   tant que le serveur tourne : macOS attribue ainsi l'accès aux fichiers (« Accès complet au disque »)
 *   à Hermes Map.app.
 * - Quand on relance l'app alors qu'elle tourne déjà (Spotlight, Dock, Finder), macOS ne démarre pas
 *   un 2e exemplaire : il envoie « reopen » à celui-ci. On y répond en relançant launch.zsh, qui rouvre
 *   la page (ou redémarre le serveur s'il est plus ancien que l'app installée).
 *
 * Aucune dépendance de compilation : AppKit et le runtime Objective-C sont chargés à l'exécution
 * (dlopen), ce qui permet de compiler avec un simple compilateur C. Si AppKit n'est pas disponible,
 * le lanceur se contente d'attendre la fin du serveur.
 */
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <pthread.h>
#include <spawn.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

typedef void *id;
typedef void *SEL;
typedef void *Class;
typedef void (*IMP)(void);

static char script[PATH_MAX + 64];
static void *objc_send;
static SEL (*sel_reg)(const char *);

static pid_t run_script(const char *arg) {
    char *argv[] = {"/bin/zsh", script, (char *)arg, NULL};
    pid_t pid = 0;
    if (posix_spawn(&pid, "/bin/zsh", NULL, NULL, argv, environ) != 0) return 0;
    return pid;
}

/* attend tous les processus enfants ; quitte l'app quand il n'y en a plus */
static void *reaper(void *unused) {
    (void)unused;
    int last = 0;
    for (;;) {
        int st = 0;
        pid_t p = waitpid(-1, &st, 0);
        if (p > 0) {
            last = WIFEXITED(st) ? WEXITSTATUS(st) : 1;
            continue;
        }
        if (errno == ECHILD) break;
        if (errno != EINTR) break;
    }
    exit(last);
    return NULL;
}

/* -[HMDelegate applicationShouldHandleReopen:hasVisibleWindows:] */
static bool on_reopen(id self, SEL cmd, id sender, bool flag) {
    (void)self; (void)cmd; (void)sender; (void)flag;
    run_script("reopen");
    return true;
}

int main(void) {
    char exe[PATH_MAX], real[PATH_MAX];
    uint32_t n = sizeof exe;
    if (_NSGetExecutablePath(exe, &n) != 0 || !realpath(exe, real)) return 1;
    char *p = strrchr(real, '/'); if (!p) return 1; *p = 0;      /* .../Contents/MacOS */
    p = strrchr(real, '/'); if (!p) return 1; *p = 0;            /* .../Contents */
    snprintf(script, sizeof script, "%s/Resources/launch.zsh", real);

    if (!run_script(NULL)) return 1;
    pthread_t t;
    pthread_create(&t, NULL, reaper, NULL);

    /* boucle d'événements AppKit, pour recevoir « reopen » */
    void *objc = dlopen("/usr/lib/libobjc.A.dylib", RTLD_NOW);
    void *appkit = dlopen("/System/Library/Frameworks/AppKit.framework/AppKit", RTLD_NOW);
    Class (*get_class)(const char *) = objc ? dlsym(objc, "objc_getClass") : NULL;
    Class (*alloc_pair)(Class, const char *, size_t) = objc ? dlsym(objc, "objc_allocateClassPair") : NULL;
    void (*reg_pair)(Class) = objc ? dlsym(objc, "objc_registerClassPair") : NULL;
    bool (*add_method)(Class, SEL, IMP, const char *) = objc ? dlsym(objc, "class_addMethod") : NULL;
    objc_send = objc ? dlsym(objc, "objc_msgSend") : NULL;
    sel_reg = objc ? dlsym(objc, "sel_registerName") : NULL;
    if (appkit && get_class && alloc_pair && reg_pair && add_method && objc_send && sel_reg) {
        id (*send0)(id, SEL) = (id (*)(id, SEL))objc_send;
        void (*send1)(id, SEL, id) = (void (*)(id, SEL, id))objc_send;
        Class nsapp_class = get_class("NSApplication");
        Class nsobject = get_class("NSObject");
        if (nsapp_class && nsobject) {
            id app = send0((id)nsapp_class, sel_reg("sharedApplication"));
            Class dcls = alloc_pair(nsobject, "HMDelegate", 0);
            if (app && dcls) {
                add_method(dcls, sel_reg("applicationShouldHandleReopen:hasVisibleWindows:"), (IMP)on_reopen, "c@:@c");
                reg_pair(dcls);
                id delegate = send0(send0((id)dcls, sel_reg("alloc")), sel_reg("init"));
                send1(app, sel_reg("setDelegate:"), delegate);
                send0(app, sel_reg("run"));          /* ne revient pas ; le thread « reaper » quitte l'app */
            }
        }
    }
    pthread_join(t, NULL);                           /* sans AppKit : on attend simplement */
    return 0;
}
