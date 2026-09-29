/*
 * Minimal stdio + printf family for wasm32-unknown-unknown.
 *
 * - FILE streams are no-op stubs (Lua's `print` goes through fwrite(stdout)).
 * - The printf family is implemented here in C (variadic access is C-only);
 *   float conversions (%f/%e/%g) delegate the rounding/rendering to the Rust
 *   helper `__shim_double_to_str`.
 */

#include <stddef.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

/* Rust-provided: render a double per C printf float semantics. */
extern int __shim_double_to_str(char *buf, size_t cap, double v,
                                int conv, int prec, int alt, int force_sign);

typedef struct wasm_file {
    int dummy;
} wasm_file_t;

static wasm_file_t _stdin_ = {0};
static wasm_file_t _stdout_ = {0};
static wasm_file_t _stderr_ = {0};

FILE *stdin = &_stdin_;
FILE *stdout = &_stdout_;
FILE *stderr = &_stderr_;

size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *stream) {
    (void)ptr;
    (void)stream;
    return size * nmemb;
}

size_t fread(void *ptr, size_t size, size_t nmemb, FILE *stream) {
    (void)ptr; (void)size; (void)nmemb; (void)stream;
    return 0;
}

int fputs(const char *s, FILE *stream) {
    (void)s;
    (void)stream;
    return 1;
}

int fputc(int c, FILE *stream) {
    (void)stream;
    return c;
}

int putchar(int c) {
    return c;
}

int puts(const char *s) {
    (void)s;
    return 0;
}

int fflush(FILE *stream) {
    (void)stream;
    return 0;
}

/* io-lib stubs (liolib.c is excluded from the wasm build; kept for safety). */
FILE *fopen(const char *path, const char *mode) {
    (void)path; (void)mode;
    return 0;
}
FILE *freopen(const char *path, const char *mode, FILE *stream) {
    (void)path; (void)mode; (void)stream;
    return 0;
}
int fclose(FILE *stream) { (void)stream; return 0; }
int fseek(FILE *stream, long offset, int whence) {
    (void)stream; (void)offset; (void)whence;
    return -1;
}
long ftell(FILE *stream) { (void)stream; return -1; }
int feof(FILE *stream) { (void)stream; return 0; }
int ferror(FILE *stream) { (void)stream; return 0; }
int fgetc(FILE *stream) { (void)stream; return EOF; }
int getc(FILE *stream) { (void)stream; return EOF; }
int ungetc(int c, FILE *stream) { (void)c; (void)stream; return EOF; }
char *fgets(char *s, int size, FILE *stream) {
    (void)s; (void)size; (void)stream;
    return 0;
}
int remove(const char *path) { (void)path; return -1; }
int rename(const char *oldpath, const char *newpath) {
    (void)oldpath; (void)newpath;
    return -1;
}

/* ------------------------------------------------------------------ */
/* vsnprintf core                                                      */
/* ------------------------------------------------------------------ */

struct fmt {
    int left;     /* '-' */
    int plus;     /* '+' */
    int space;    /* ' ' */
    int alt;      /* '#' */
    int zero;     /* '0' */
    int width;
    int have_prec;
    int prec;
    int llong;    /* 0=int, 1=long, 2=long long */
};

static size_t emit(char *str, size_t size, const char *s, size_t n) {
    size_t i;
    for (i = 0; i < n; i++)
        if (i < size) str[i] = s[i];
    return n;
}

static size_t emit_pad(char *str, size_t size, char c, size_t n) {
    size_t i;
    for (i = 0; i < n; i++)
        if (i < size) str[i] = c;
    return n;
}

/* Render a zero-terminated-ish fixed string with width handling. */
static size_t render_str(char *str, size_t size, const char *s, size_t len,
                         const struct fmt *f) {
    size_t total = (size_t)f->width > len ? (size_t)f->width : len;
    size_t pad = total - len;
    size_t n = 0;
    if (!f->left) n += emit_pad(str + n, size > n ? size - n : 0, ' ', pad);
    n += emit(str + n, size > n ? size - n : 0, s, len);
    if (f->left) n += emit_pad(str + n, size > n ? size - n : 0, ' ', pad);
    return n;
}

/* Render an integer with sign/prefix/precision/width. */
static size_t render_int(char *str, size_t size, unsigned long long val,
                         int neg, const struct fmt *f, int base, int upper) {
    char digits[64];
    int nd = 0;
    char sign = 0;
    char prefix[3];
    size_t preflen = 0;
    size_t zp = 0;
    size_t i, total, pad, n = 0;
    int vzero = (val == 0);

    if (vzero && f->have_prec && f->prec == 0) {
        nd = 0;
    } else {
        do {
            int d = (int)(val % (unsigned)base);
            digits[nd++] = (char)((d < 10) ? ('0' + d)
                                           : (upper ? 'A' : 'a') + (d - 10));
            val /= (unsigned)base;
        } while (val);
    }

    if (neg) sign = '-';
    else if (f->plus) sign = '+';
    else if (f->space) sign = ' ';

    if (f->alt && !vzero) {
        if (base == 16) {
            prefix[0] = '0';
            prefix[1] = upper ? 'X' : 'x';
            preflen = 2;
        } else if (base == 8) {
            prefix[0] = '0';
            preflen = 1;
        }
    }

    if (f->have_prec && f->prec > nd)
        zp = (size_t)(f->prec - nd);

    total = (sign ? 1 : 0) + preflen + zp + (size_t)nd;
    pad = (size_t)f->width > total ? (size_t)f->width - total : 0;

    if (!f->left) {
        if (f->zero && !f->have_prec) {
            /* zeros go after sign and prefix */
            if (sign) n += emit(str + n, size > n ? size - n : 0, &sign, 1);
            if (preflen) n += emit(str + n, size > n ? size - n : 0, prefix, preflen);
            n += emit_pad(str + n, size > n ? size - n : 0, '0', pad);
        } else {
            n += emit_pad(str + n, size > n ? size - n : 0, ' ', pad);
            if (sign) n += emit(str + n, size > n ? size - n : 0, &sign, 1);
            if (preflen) n += emit(str + n, size > n ? size - n : 0, prefix, preflen);
        }
    } else {
        if (sign) n += emit(str + n, size > n ? size - n : 0, &sign, 1);
        if (preflen) n += emit(str + n, size > n ? size - n : 0, prefix, preflen);
    }

    n += emit_pad(str + n, size > n ? size - n : 0, '0', zp);
    for (i = (size_t)nd; i > 0; i--)
        n += emit(str + n, size > n ? size - n : 0, &digits[i - 1], 1);
    if (f->left) n += emit_pad(str + n, size > n ? size - n : 0, ' ', pad);
    return n;
}

/* Render a float produced by the Rust helper, with width/zero padding. */
static size_t render_fp(char *str, size_t size, const char *s, size_t len,
                        const struct fmt *f) {
    size_t total = (size_t)f->width > len ? (size_t)f->width : len;
    size_t pad = total - len;
    size_t n = 0;
    if (!f->left) {
        if (f->zero) {
            /* sign goes first, then zeros */
            if (len > 0 && (s[0] == '-' || s[0] == '+' || s[0] == ' ')) {
                n += emit(str + n, size > n ? size - n : 0, s, 1);
                s++;
                len--;
            }
            n += emit_pad(str + n, size > n ? size - n : 0, '0', pad);
        } else {
            n += emit_pad(str + n, size > n ? size - n : 0, ' ', pad);
        }
    }
    n += emit(str + n, size > n ? size - n : 0, s, len);
    if (f->left) n += emit_pad(str + n, size > n ? size - n : 0, ' ', pad);
    return n;
}

int vsnprintf(char *str, size_t size, const char *fmt, va_list ap) {
    const char *p = fmt;
    size_t out = 0;

    while (*p) {
        struct fmt f;
        char c;

        if (*p != '%') {
            out += emit(str, size, p, 1);
            p++;
            continue;
        }
        p++;

        f.left = f.plus = f.space = f.alt = f.zero = 0;
        f.width = 0;
        f.have_prec = 0;
        f.prec = 0;
        f.llong = 0;

        /* flags */
        for (;;) {
            if (*p == '-') f.left = 1;
            else if (*p == '+') f.plus = 1;
            else if (*p == ' ') f.space = 1;
            else if (*p == '#') f.alt = 1;
            else if (*p == '0') f.zero = 1;
            else break;
            p++;
        }
        /* width */
        if (*p == '*') {
            int w = va_arg(ap, int);
            if (w < 0) { f.left = 1; w = -w; }
            f.width = w;
            p++;
        } else {
            while (*p >= '0' && *p <= '9') {
                f.width = f.width * 10 + (*p - '0');
                p++;
            }
        }
        /* precision */
        if (*p == '.') {
            p++;
            f.have_prec = 1;
            if (*p == '*') {
                int pr = va_arg(ap, int);
                if (pr < 0) f.have_prec = 0;
                else f.prec = pr;
                p++;
            } else {
                while (*p >= '0' && *p <= '9') {
                    f.prec = f.prec * 10 + (*p - '0');
                    p++;
                }
            }
        }
        /* length modifiers */
        if (*p == 'h') {
            p++;
            if (*p == 'h') p++;
            f.llong = 0;
        } else if (*p == 'l') {
            p++;
            if (*p == 'l') { f.llong = 2; p++; }
            else f.llong = 1;
        } else if (*p == 'z' || *p == 't') {
            f.llong = 1;
            p++;
        }

        c = *p;
        if (c == '\0') break;

        if (c == '%') {
            out += emit(str, size, "%", 1);
        } else if (c == 'd' || c == 'i') {
            long long v;
            if (f.llong == 2) v = (long long)va_arg(ap, long long);
            else if (f.llong == 1) v = (long long)va_arg(ap, long);
            else v = (long long)va_arg(ap, int);
            out += render_int(str, size,
                              v < 0 ? (unsigned long long)(-v)
                                    : (unsigned long long)v,
                              v < 0, &f, 10, 0);
        } else if (c == 'u' || c == 'o' || c == 'x' || c == 'X') {
            unsigned long long uv;
            if (f.llong == 2) uv = (unsigned long long)va_arg(ap, unsigned long long);
            else if (f.llong == 1) uv = (unsigned long long)va_arg(ap, unsigned long);
            else uv = (unsigned long long)va_arg(ap, unsigned int);
            out += render_int(str, size, uv, 0, &f,
                              c == 'o' ? 8 : (c == 'x' || c == 'X') ? 16 : 10,
                              c == 'X');
        } else if (c == 'c') {
            char ch = (char)va_arg(ap, int);
            out += render_str(str, size, &ch, 1, &f);
        } else if (c == 's') {
            const char *s = va_arg(ap, const char *);
            size_t sl;
            if (!s) s = "(null)";
            sl = strlen(s);
            if (f.have_prec && (size_t)f.prec < sl) sl = (size_t)f.prec;
            out += render_str(str, size, s, sl, &f);
        } else if (c == 'p') {
            uintptr_t pv = (uintptr_t)va_arg(ap, void *);
            char tmp[32];
            int i, nd = 0;
            uintptr_t v = pv;
            struct fmt pf = f;
            pf.llong = 2;
            do {
                int d = (int)(v % 16);
                tmp[nd++] = (char)((d < 10) ? '0' + d : 'a' + (d - 10));
                v /= 16;
            } while (v);
            /* glibc-style: 0x prefix, no zero-flag interplay */
            out += emit(str, size, "0x", 2);
            for (i = nd - 1; i >= 0; i--)
                out += emit(str + out, size > out ? size - out : 0, &tmp[i], 1);
            (void)pf;
        } else if (c == 'f' || c == 'e' || c == 'E' || c == 'g' || c == 'G') {
            double dv = va_arg(ap, double);
            char tmp[128];
            int prec, len;
            if (c == 'g' || c == 'G') {
                /* C99: default precision for %g is 6; explicit ".0" means 1. */
                prec = f.have_prec ? (f.prec == 0 ? 1 : f.prec) : 6;
            } else {
                prec = f.have_prec ? f.prec : 6;
            }
            len = __shim_double_to_str(tmp, sizeof(tmp), dv, c, prec,
                                       f.alt ? 1 : 0,
                                       f.plus ? 1 : (f.space ? 2 : 0));
            out += render_fp(str, size, tmp, (size_t)len, &f);
        } else {
            char tmp[2];
            tmp[0] = '%';
            tmp[1] = c;
            out += emit(str, size, tmp, 2);
        }
        p++;
    }

    if (size > 0) {
        if (out < size) str[out] = '\0';
        else str[size - 1] = '\0';
    }
    return (int)out;
}

int snprintf(char *str, size_t size, const char *fmt, ...) {
    va_list ap;
    int r;
    va_start(ap, fmt);
    r = vsnprintf(str, size, fmt, ap);
    va_end(ap);
    return r;
}

int sprintf(char *str, const char *fmt, ...) {
    va_list ap;
    int r;
    va_start(ap, fmt);
    r = vsnprintf(str, (size_t)-1, fmt, ap);
    va_end(ap);
    return r;
}

int vfprintf(FILE *stream, const char *fmt, va_list ap) {
    (void)stream;
    return vsnprintf(0, 0, fmt, ap);
}

int fprintf(FILE *stream, const char *fmt, ...) {
    va_list ap;
    int r;
    va_start(ap, fmt);
    r = vfprintf(stream, fmt, ap);
    va_end(ap);
    return r;
}

int vprintf(const char *fmt, va_list ap) {
    return vsnprintf(0, 0, fmt, ap);
}

int printf(const char *fmt, ...) {
    va_list ap;
    int r;
    va_start(ap, fmt);
    r = vprintf(fmt, ap);
    va_end(ap);
    return r;
}
