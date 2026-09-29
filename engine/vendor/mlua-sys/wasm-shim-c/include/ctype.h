#ifndef WASM_SHIM_CTYPE_H
#define WASM_SHIM_CTYPE_H

int isalpha(int c);
int isdigit(int c);
int isalnum(int c);
int isspace(int c);
int isupper(int c);
int islower(int c);
int isxdigit(int c);
int iscntrl(int c);
int ispunct(int c);
int isgraph(int c);
int isprint(int c);
int tolower(int c);
int toupper(int c);

#endif
