idf_component_register(
     SRCS 
          "./miniz.c" 
          "./pngle.c"

     INCLUDE_DIRS 
          "." 

     REQUIRES log freertos
)

# target_compile_options(${COMPONENT_LIB} PRIVATE -DCONFIG_I2C_SKIP_LEGACY_CONFLICT_CHECK)