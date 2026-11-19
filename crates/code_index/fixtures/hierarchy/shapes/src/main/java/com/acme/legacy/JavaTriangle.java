package com.acme.legacy;

import com.acme.shapes.Polygon;

public class JavaTriangle extends Polygon {
    @Override
    public double area() { return 0.5; }

    public int sides() { return 3; }

    public String getName() { return "triangle"; }
}
