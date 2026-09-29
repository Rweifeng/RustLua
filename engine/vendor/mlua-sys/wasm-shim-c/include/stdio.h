#ifndef WASM_SHIM_STDIO_H
#define WASM_SHIM_STDIO_H

#include <stddef.h>

typedef struct wasm_file FILE;

#define EOF (-1)

#define BUFSIZ 8192
#define L_tmpnam 20

#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

extern FILE *stdin;
extern FILE *stdout;
extern FILE *stderr;

size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *stream);
size_t fread(void *ptr, size_t size, size_t nmemb, FILE *stream);
int    fputs(const char *s, FILE *stream);
int    fputc(int c, FILE *stream);
int    putchar(int c);
int    puts(const char *s);
int    fflush(FILE *stream);

FILE *fopen(const char *path, const char *mode);
FILE *freopen(const char *path, const char *mode, FILE *stream);
int   fclose(FILE *stream);
int   fseek(FILE *stream, long offset, int whence);
long  ftell(FILE *stream);
int   feof(FILE *stream);
int   ferror(FILE *stream);
int   fgetc(FILE *stream);
int   getc(FILE *stream);
int   ungetc(int c, FILE *stream);
char *fgets(char *s, int size, FILE *stream);

int remove(const char *path);
int rename(const char *oldpath, const char *newpath);

int fprintf(FILE *stream, const char *format, ...);
int vfprintf(FILE *stream, const char *format, __builtin_va_list ap);
int printf(const char *format, ...);
int vprintf(const char *format, __builtin_va_list ap);
int sprintf(char *str, const char *format, ...);
int snprintf(char *str, size_t size, const char *format, ...);
int vsnprintf(char *str, size_t size, const char *format, __builtin_va_list ap);

#endif
