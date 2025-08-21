#ifndef TABULAR_FFI_H
#define TABULAR_FFI_H
#ifdef __cplusplus
extern "C"
{
#endif
       const char *tabular_version(void);
       int tabular_run(void); // Beware: currently desktop-oriented; may no-op / block.
#ifdef __cplusplus
}
#endif
#endif
