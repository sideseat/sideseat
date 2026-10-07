// synctrace: record the writes and syncs a process makes under one directory.
//
// Loaded into the server by scripts/check/durability.py (DYLD_INSERT_LIBRARIES on macOS, LD_PRELOAD on Linux),
// it appends one line per event to $SYNCTRACE_OUT:
//
//     <seconds.micros> <event> <path> [<path>]
//
// for every write to, sync of, rename into, or directory created under $SYNCTRACE_ROOT. The events are
// `write`, `fsync` (fsync or fdatasync), `fullfsync` (macOS F_FULLFSYNC), `barrierfsync` (macOS
// F_BARRIERFSYNC), `rename <from> <to>`, `mkdir`, and `create` (an open that created the file). Events outside the root are not recorded, so sockets,
// logs and the terminal cost one path lookup and nothing else.
//
// The driver decides what the events prove; this file only reports them faithfully.

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <unistd.h>

#ifdef __APPLE__
#include <sys/param.h>
#else
#include <dlfcn.h>
#endif

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif

// Recording writes the trace file, which would otherwise record itself.
static __thread int in_trace = 0;

static int under_root(const char *path) {
  const char *root = getenv("SYNCTRACE_ROOT");
  if (!root || !path) return 0;
  size_t n = strlen(root);
  return strncmp(path, root, n) == 0 && (path[n] == '/' || path[n] == '\0');
}

static int fd_path(int fd, char *out) {
#ifdef __APPLE__
  return fcntl(fd, F_GETPATH, out) == 0;
#else
  char link[64];
  snprintf(link, sizeof link, "/proc/self/fd/%d", fd);
  ssize_t len = readlink(link, out, PATH_MAX - 1);
  if (len < 0) return 0;
  out[len] = '\0';
  return 1;
#endif
}

// An absolute path for a name passed to rename or mkdir, resolving the directory but not the last component,
// which may not exist yet.
static int absolute(const char *name, char *out) {
  char dir[PATH_MAX];
  const char *slash = strrchr(name, '/');
  if (slash) {
    size_t len = (size_t)(slash - name);
    if (len == 0) len = 1;
    if (len >= sizeof dir) return 0;
    memcpy(dir, name, len);
    dir[len] = '\0';
  } else {
    strcpy(dir, ".");
  }
  char resolved[PATH_MAX];
  if (!realpath(dir, resolved)) return 0;
  int len = snprintf(out, PATH_MAX, "%s/%s", resolved, slash ? slash + 1 : name);
  return len > 0 && len < PATH_MAX;
}

static void emit(const char *event, const char *path, const char *second) {
  if (in_trace) return;
  in_trace = 1;
  int saved = errno;
  const char *out = getenv("SYNCTRACE_OUT");
  if (out) {
    FILE *f = fopen(out, "a");
    if (f) {
      struct timeval tv;
      gettimeofday(&tv, NULL);
      if (second)
        fprintf(f, "%ld.%06d %s %s %s\n", (long)tv.tv_sec, (int)tv.tv_usec, event, path, second);
      else
        fprintf(f, "%ld.%06d %s %s\n", (long)tv.tv_sec, (int)tv.tv_usec, event, path);
      fclose(f);
    }
  }
  errno = saved;
  in_trace = 0;
}

static void note_fd(const char *event, int fd) {
  if (in_trace) return;
  char path[PATH_MAX];
  if (fd_path(fd, path) && under_root(path)) emit(event, path, NULL);
}

static void note_rename(const char *from, const char *to) {
  if (in_trace) return;
  char a[PATH_MAX], b[PATH_MAX];
  if (absolute(from, a) && absolute(to, b) && under_root(b)) emit("rename", a, b);
}

// Whether `name` exists, asked before an O_CREAT open so the open can be reported as a creation.
static int exists(const char *name) {
  struct stat st;
  return stat(name, &st) == 0;
}

static void note_create(int fd, int existed) {
  if (in_trace || fd < 0 || existed) return;
  char path[PATH_MAX];
  if (fd_path(fd, path) && under_root(path)) emit("create", path, NULL);
}

static void note_mkdir(const char *name, int result) {
  if (in_trace || result != 0) return;
  char path[PATH_MAX];
  if (absolute(name, path) && under_root(path)) emit("mkdir", path, NULL);
}

#ifdef __APPLE__
// dyld interposition: calls from this image reach the originals, every other image reaches these.
#define INTERPOSE(replacement, original)                                                                \
  __attribute__((used)) static struct {                                                                 \
    const void *r;                                                                                      \
    const void *o;                                                                                      \
  } interpose_##original __attribute__((section("__DATA,__interpose"))) = {                            \
      (const void *)(unsigned long)&replacement, (const void *)(unsigned long)&original};

extern int fdatasync(int);

static int st_fsync(int fd) { int r = fsync(fd); if (r == 0) note_fd("fsync", fd); return r; }
INTERPOSE(st_fsync, fsync)
static int st_fdatasync(int fd) { int r = fdatasync(fd); if (r == 0) note_fd("fsync", fd); return r; }
INTERPOSE(st_fdatasync, fdatasync)
static int st_fcntl(int fd, int cmd, ...) {
  va_list ap;
  va_start(ap, cmd);
  void *arg = va_arg(ap, void *);
  va_end(ap);
  int r = fcntl(fd, cmd, arg);
  if (r != -1 && cmd == F_FULLFSYNC) note_fd("fullfsync", fd);
  if (r != -1 && cmd == 85 /* F_BARRIERFSYNC */) note_fd("barrierfsync", fd);
  return r;
}
INTERPOSE(st_fcntl, fcntl)
static ssize_t st_write(int fd, const void *b, size_t n) { ssize_t r = write(fd, b, n); if (r > 0) note_fd("write", fd); return r; }
INTERPOSE(st_write, write)
static ssize_t st_pwrite(int fd, const void *b, size_t n, off_t o) { ssize_t r = pwrite(fd, b, n, o); if (r > 0) note_fd("write", fd); return r; }
INTERPOSE(st_pwrite, pwrite)
static ssize_t st_writev(int fd, const struct iovec *v, int c) { ssize_t r = writev(fd, v, c); if (r > 0) note_fd("write", fd); return r; }
INTERPOSE(st_writev, writev)
static int st_rename(const char *a, const char *b) { int r = rename(a, b); if (r == 0) note_rename(a, b); return r; }
INTERPOSE(st_rename, rename)
static int st_renameat(int fa, const char *a, int fb, const char *b) { int r = renameat(fa, a, fb, b); if (r == 0) note_rename(a, b); return r; }
INTERPOSE(st_renameat, renameat)
static int st_mkdir(const char *p, mode_t m) { int r = mkdir(p, m); note_mkdir(p, r); return r; }
INTERPOSE(st_mkdir, mkdir)
static int st_open(const char *p, int flags, ...) {
  mode_t mode = 0;
  if (flags & O_CREAT) { va_list ap; va_start(ap, flags); mode = (mode_t)va_arg(ap, int); va_end(ap); }
  int existed = (flags & O_CREAT) ? exists(p) : 1;
  int fd = open(p, flags, mode);
  note_create(fd, existed);
  return fd;
}
INTERPOSE(st_open, open)

#else
// LD_PRELOAD: these definitions shadow libc's, which they reach through RTLD_NEXT.
#define REAL(name) static __typeof__(name) *real_##name; if (!real_##name) real_##name = dlsym(RTLD_NEXT, #name)

int fsync(int fd) { REAL(fsync); int r = real_fsync(fd); if (r == 0) note_fd("fsync", fd); return r; }
int fdatasync(int fd) { REAL(fdatasync); int r = real_fdatasync(fd); if (r == 0) note_fd("fsync", fd); return r; }
ssize_t write(int fd, const void *b, size_t n) { REAL(write); ssize_t r = real_write(fd, b, n); if (r > 0) note_fd("write", fd); return r; }
ssize_t pwrite(int fd, const void *b, size_t n, off_t o) { REAL(pwrite); ssize_t r = real_pwrite(fd, b, n, o); if (r > 0) note_fd("write", fd); return r; }
ssize_t pwrite64(int fd, const void *b, size_t n, off_t o) { REAL(pwrite64); ssize_t r = real_pwrite64(fd, b, n, o); if (r > 0) note_fd("write", fd); return r; }
ssize_t writev(int fd, const struct iovec *v, int c) { REAL(writev); ssize_t r = real_writev(fd, v, c); if (r > 0) note_fd("write", fd); return r; }
ssize_t pwritev(int fd, const struct iovec *v, int c, off_t o) { REAL(pwritev); ssize_t r = real_pwritev(fd, v, c, o); if (r > 0) note_fd("write", fd); return r; }
int rename(const char *a, const char *b) { REAL(rename); int r = real_rename(a, b); if (r == 0) note_rename(a, b); return r; }
int renameat(int fa, const char *a, int fb, const char *b) { REAL(renameat); int r = real_renameat(fa, a, fb, b); if (r == 0) note_rename(a, b); return r; }
int mkdir(const char *p, mode_t m) { REAL(mkdir); int r = real_mkdir(p, m); note_mkdir(p, r); return r; }

#define OPEN_LIKE(name, ...)                                                                            \
  mode_t mode = 0;                                                                                      \
  if (flags & O_CREAT) { va_list ap; va_start(ap, flags); mode = (mode_t)va_arg(ap, int); va_end(ap); } \
  int existed = (flags & O_CREAT) ? exists(p) : 1;                                                      \
  REAL(name);                                                                                           \
  int fd = real_##name(__VA_ARGS__);                                                                    \
  note_create(fd, existed);                                                                          \
  return fd;

int open(const char *p, int flags, ...) { OPEN_LIKE(open, p, flags, mode) }
int open64(const char *p, int flags, ...) { OPEN_LIKE(open64, p, flags, mode) }
// `existed` is only exact for an absolute name or one relative to the working directory; the server opens its
// data files by absolute path.
int openat(int dirfd, const char *p, int flags, ...) { OPEN_LIKE(openat, dirfd, p, flags, mode) }
int openat64(int dirfd, const char *p, int flags, ...) { OPEN_LIKE(openat64, dirfd, p, flags, mode) }
#endif
