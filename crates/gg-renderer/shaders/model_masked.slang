// The model shader for masked materials: model.slang, discarding what's less opaque than the
// material's cutoff, and returning an alpha of 1 for the rest.

#define MASKED
#include "model.slang"
