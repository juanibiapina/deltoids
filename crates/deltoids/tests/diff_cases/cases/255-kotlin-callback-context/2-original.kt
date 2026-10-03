fun greetAll(names: List<String>) {
    println("Starting")
    names.forEach {
        println("Hello, $it")
    }
    println("Finished")
}
