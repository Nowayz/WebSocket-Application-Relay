// Linux probe only: fail one accepted native socket at an explicitly armed
// syscall. No library is loaded into ordinary servers or performance runs.
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/tcp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <unistd.h>

static int (*real_setsockopt)(int, int, int, const void *, socklen_t);
static int (*real_epoll_ctl)(int, int, int, struct epoll_event *);
static int (*real_close)(int);
static char trigger[4096], closing[4096], release_close[4096], events[4096];
static unsigned short port;
static int registration;

__attribute__((constructor)) static void initialize(void) {
    real_setsockopt = dlsym(RTLD_NEXT, "setsockopt");
    real_epoll_ctl = dlsym(RTLD_NEXT, "epoll_ctl");
    real_close = dlsym(RTLD_NEXT, "close");
    const char *directory = getenv("WAR_PROBE_DIRECTORY");
    const char *target = getenv("WAR_PROBE_PORT");
    const char *mode = getenv("WAR_PROBE_MODE");
    if (!directory || !target || !mode || !real_setsockopt || !real_epoll_ctl || !real_close)
        _exit(110);
    if (snprintf(trigger, sizeof trigger, "%s/trigger", directory) >= (int)sizeof trigger ||
        snprintf(closing, sizeof closing, "%s/closing", directory) >= (int)sizeof closing ||
        snprintf(release_close, sizeof release_close, "%s/release", directory) >= (int)sizeof release_close ||
        snprintf(events, sizeof events, "%s/events", directory) >= (int)sizeof events)
        _exit(111);
    port = (unsigned short)strtoul(target, NULL, 10);
    registration = strcmp(mode, "register") == 0;
}

static int is_target(int fd) {
    struct sockaddr_in local, peer;
    socklen_t size = sizeof local;
    return getsockname(fd, (struct sockaddr *)&local, &size) == 0 &&
        local.sin_family == AF_INET && ntohs(local.sin_port) == port &&
        (size = sizeof peer, getpeername(fd, (struct sockaddr *)&peer, &size)) == 0;
}

static void note(const char *message) {
    int fd = open(events, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
    if (fd < 0 || write(fd, message, strlen(message)) != (ssize_t)strlen(message))
        _exit(112);
    real_close(fd);
}

int setsockopt(int fd, int level, int option, const void *value, socklen_t size) {
    if (!registration && level == IPPROTO_TCP && option == TCP_NODELAY &&
        is_target(fd) && unlink(trigger) == 0) {
        note("TCP_NODELAY ENOBUFS\n");
        errno = ENOBUFS;
        return -1;
    }
    return real_setsockopt(fd, level, option, value, size);
}

int epoll_ctl(int epoll, int operation, int fd, struct epoll_event *event) {
    if (registration && operation == EPOLL_CTL_ADD && is_target(fd) && unlink(trigger) == 0) {
        note("EPOLL_CTL_ADD ENOMEM\n");
        errno = ENOMEM;
        return -1;
    }
    return real_epoll_ctl(epoll, operation, fd, event);
}

int close(int fd) {
    // The armed socket is the next native close after the failed setup call.
    // Hold its actual descriptor open while management observes admission.
    if (is_target(fd) && access(events, F_OK) == 0 && access(release_close, F_OK) != 0) {
        int gate = open(closing, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
        if (gate >= 0) {
            real_close(gate);
            for (int attempt = 0; attempt < 500 && access(release_close, F_OK) != 0; ++attempt)
                usleep(10000);
            if (access(release_close, F_OK) != 0)
                _exit(113);
        }
    }
    return real_close(fd);
}
