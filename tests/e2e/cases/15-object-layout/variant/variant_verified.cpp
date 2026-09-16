#include <variant>

struct State {
    std::variant<unsigned long long, long long> choice;
    unsigned long long count;
};

unsigned long long increment_choice(State* p) {
    auto& value = std::get<0>(p->choice);
    value += 1;
    return value;
}