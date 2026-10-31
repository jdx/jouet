public class App {
    public static void main(String[] args) {
        Greeter greeter = new Greeter();
        greeter.greet("world");
    }
}

class Greeter {
    void greet(String name) {
        System.out.println("hello " + name);
    }
}
