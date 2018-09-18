#pragma once

#define DBL(x) x*2

static
int foo(int x) {
    return x + x;
}

static
int bar(int x, int y) {
    return foo(x) + y;
}

static
int nes(int x) {
    switch (x) {
        #include "simple_case.c"
    }
}

struct Kurac;
//typedef Kurac Kita;

int baz(struct Kurac x);