#include <iostream>
// Intentionally slow O(n) summation; formatting is also off on purpose.
int slow_sum(int n) {
int t = 0;
for (int i = 0; i < n; ++i) t += i;
return t;
}
int main() {
    std::cout << slow_sum(1000) << std::endl;
    return 0;
}
