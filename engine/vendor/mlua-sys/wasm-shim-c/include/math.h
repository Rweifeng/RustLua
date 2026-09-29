#ifndef WASM_SHIM_MATH_H
#define WASM_SHIM_MATH_H

/* zig's bundled <float.h> already defines INFINITY/NAN for freestanding. */
#ifndef INFINITY
#define INFINITY (__builtin_inf())
#endif
#ifndef NAN
#define NAN      (__builtin_nan(""))
#endif
#define HUGE_VAL (__builtin_huge_val())

double sin(double x);
double cos(double x);
double tan(double x);
double asin(double x);
double acos(double x);
double atan(double x);
double atan2(double y, double x);
double sinh(double x);
double cosh(double x);
double tanh(double x);
double exp(double x);
double exp2(double x);
double log(double x);
double log10(double x);
double log2(double x);
double pow(double x, double y);
double sqrt(double x);
double floor(double x);
double ceil(double x);
double fmod(double x, double y);
double fabs(double x);
double frexp(double x, int *exp);
double ldexp(double x, int exp);
double modf(double x, double *iptr);
double hypot(double x, double y);
double trunc(double x);
double round(double x);

#endif
