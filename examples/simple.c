#include "simple.h"
#include <stdio.h>
#include <math.h>
#include <string.h>

int main() {
    int x = nes(1);
    int len = strlen("kurcina") + x;
    printf("%d\n", len);
    return x;
}