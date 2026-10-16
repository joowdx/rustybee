# kotlinx.serialization: keep generated serializers for @Serializable protocol types.
-keepattributes RuntimeVisibleAnnotations,AnnotationDefault
-keepclassmembers @kotlinx.serialization.Serializable class dev.joowdx.rustybee.** {
    *** Companion;
    kotlinx.serialization.KSerializer serializer(...);
}
-keepclasseswithmembers class dev.joowdx.rustybee.** {
    kotlinx.serialization.KSerializer serializer(...);
}
