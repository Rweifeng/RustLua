#ifndef WASM_SHIM_STDLIB_H
#define WASM_SHIM_STDLIB_H

#include <stddef.h>

#ifndef NULL
#define NULL ((void *)0)
#endif

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1

void *malloc(size_t size);
void *calloc(size_t nmemb, size_t size);
void *realloc(void *ptr, size_t size);
void  free(void *ptr);

void qsort(void *base, size_t nmemb, size_t size,
           int (*compar)(const void *, const void *));

double  strtod(const char *nptr, char **endptr);
long    strtol(const char *nptr, char **endptr, int base);
unsigned long strtoul(const char *nptr, char **endptr, int base);

int    atoi(const char *nptr);
double atof(const char *nptr);
int    abs(int j);

char *getenv(const char *name);
int   system(const char *command);

void exit(int status) __attribute__((noreturn));
void abort(void) __attribute__((noreturn));

#endif
