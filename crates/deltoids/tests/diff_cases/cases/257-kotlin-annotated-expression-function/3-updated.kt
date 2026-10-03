/** Format a greeting. */
@Deprecated("Use greet instead")
fun String.`friendly greeting`() =
    "Welcome, $this"
