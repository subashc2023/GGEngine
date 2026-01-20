#pragma once

// From glfw3.h - using GLFW key codes as the standard for cross-platform consistency

#define GG_KEY_UNKNOWN            -1

#define GG_KEY_SPACE              32
#define GG_KEY_APOSTROPHE         39  /* ' */
#define GG_KEY_COMMA              44  /* , */
#define GG_KEY_MINUS              45  /* - */
#define GG_KEY_PERIOD             46  /* . */
#define GG_KEY_SLASH              47  /* / */
#define GG_KEY_0                  48
#define GG_KEY_1                  49
#define GG_KEY_2                  50
#define GG_KEY_3                  51
#define GG_KEY_4                  52
#define GG_KEY_5                  53
#define GG_KEY_6                  54
#define GG_KEY_7                  55
#define GG_KEY_8                  56
#define GG_KEY_9                  57
#define GG_KEY_SEMICOLON          59  /* ; */
#define GG_KEY_EQUAL              61  /* = */
#define GG_KEY_A                  65
#define GG_KEY_B                  66
#define GG_KEY_C                  67
#define GG_KEY_D                  68
#define GG_KEY_E                  69
#define GG_KEY_F                  70
#define GG_KEY_G                  71
#define GG_KEY_H                  72
#define GG_KEY_I                  73
#define GG_KEY_J                  74
#define GG_KEY_K                  75
#define GG_KEY_L                  76
#define GG_KEY_M                  77
#define GG_KEY_N                  78
#define GG_KEY_O                  79
#define GG_KEY_P                  80
#define GG_KEY_Q                  81
#define GG_KEY_R                  82
#define GG_KEY_S                  83
#define GG_KEY_T                  84
#define GG_KEY_U                  85
#define GG_KEY_V                  86
#define GG_KEY_W                  87
#define GG_KEY_X                  88
#define GG_KEY_Y                  89
#define GG_KEY_Z                  90
#define GG_KEY_LEFT_BRACKET       91  /* [ */
#define GG_KEY_BACKSLASH          92  /* \ */
#define GG_KEY_RIGHT_BRACKET      93  /* ] */
#define GG_KEY_GRAVE_ACCENT       96  /* ` */
#define GG_KEY_WORLD_1            161 /* non-US #1 */
#define GG_KEY_WORLD_2            162 /* non-US #2 */

/* Function keys */
#define GG_KEY_ESCAPE             256
#define GG_KEY_ENTER              257
#define GG_KEY_TAB                258
#define GG_KEY_BACKSPACE          259
#define GG_KEY_INSERT             260
#define GG_KEY_DELETE             261
#define GG_KEY_RIGHT              262
#define GG_KEY_LEFT               263
#define GG_KEY_DOWN               264
#define GG_KEY_UP                 265
#define GG_KEY_PAGE_UP            266
#define GG_KEY_PAGE_DOWN          267
#define GG_KEY_HOME               268
#define GG_KEY_END                269
#define GG_KEY_CAPS_LOCK          280
#define GG_KEY_SCROLL_LOCK        281
#define GG_KEY_NUM_LOCK           282
#define GG_KEY_PRINT_SCREEN       283
#define GG_KEY_PAUSE              284
#define GG_KEY_F1                 290
#define GG_KEY_F2                 291
#define GG_KEY_F3                 292
#define GG_KEY_F4                 293
#define GG_KEY_F5                 294
#define GG_KEY_F6                 295
#define GG_KEY_F7                 296
#define GG_KEY_F8                 297
#define GG_KEY_F9                 298
#define GG_KEY_F10                299
#define GG_KEY_F11                300
#define GG_KEY_F12                301
#define GG_KEY_F13                302
#define GG_KEY_F14                303
#define GG_KEY_F15                304
#define GG_KEY_F16                305
#define GG_KEY_F17                306
#define GG_KEY_F18                307
#define GG_KEY_F19                308
#define GG_KEY_F20                309
#define GG_KEY_F21                310
#define GG_KEY_F22                311
#define GG_KEY_F23                312
#define GG_KEY_F24                313
#define GG_KEY_F25                314
#define GG_KEY_KP_0               320
#define GG_KEY_KP_1               321
#define GG_KEY_KP_2               322
#define GG_KEY_KP_3               323
#define GG_KEY_KP_4               324
#define GG_KEY_KP_5               325
#define GG_KEY_KP_6               326
#define GG_KEY_KP_7               327
#define GG_KEY_KP_8               328
#define GG_KEY_KP_9               329
#define GG_KEY_KP_DECIMAL         330
#define GG_KEY_KP_DIVIDE          331
#define GG_KEY_KP_MULTIPLY        332
#define GG_KEY_KP_SUBTRACT        333
#define GG_KEY_KP_ADD             334
#define GG_KEY_KP_ENTER           335
#define GG_KEY_KP_EQUAL           336
#define GG_KEY_LEFT_SHIFT         340
#define GG_KEY_LEFT_CONTROL       341
#define GG_KEY_LEFT_ALT           342
#define GG_KEY_LEFT_SUPER         343
#define GG_KEY_RIGHT_SHIFT        344
#define GG_KEY_RIGHT_CONTROL      345
#define GG_KEY_RIGHT_ALT          346
#define GG_KEY_RIGHT_SUPER        347
#define GG_KEY_MENU               348

#define GG_KEY_LAST               GG_KEY_MENU
