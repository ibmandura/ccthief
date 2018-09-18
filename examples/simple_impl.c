
#include "simple.h"

struct Kurac {
    int kita;
    int picka;
};

int baz(struct Kurac x) {
    int bar = foo(x.kita);
    int br = foo(x.picka);
    return bar + br;
}
