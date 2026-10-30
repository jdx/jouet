namespace shapes {

class Circle {
public:
    explicit Circle(double r) : radius(r) {}
    double area() const;

private:
    double radius;
};

double Circle::area() const {
    return 3.14159 * radius * radius;
}

}  // namespace shapes
