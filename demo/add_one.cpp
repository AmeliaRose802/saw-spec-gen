#include <cstdint>
#include <cstdio>
#include <cstdlib>

int super_important = 7;

// // An enum with known variants — the tool should constrain discriminant values
// enum class Status : uint32_t {
//     Ok = 0,
//     Warning = 1,
//     Error = 2,
// };

// // A concrete class (NOT going through an interface) — the tool should use
// // llvm_verify (real impl) not llvm_unsafe_assume_spec (havoc override)
// class Validator {
//     uint32_t threshold_;
// public:
//     Validator(uint32_t t) : threshold_(t) {}

//     Status check(uint32_t value) const {
//         if (value > threshold_) return Status::Error;
//         if (value > threshold_ / 2) return Status::Warning;
//         return Status::Ok;
//     }
// };

class ILog {
public:
    virtual void log(const char* message) = 0;
    virtual int add_one(uint32_t x) = 0;
    virtual void sus(int * in) = 0;
};

class OkLog : public ILog {
    
public:

    virtual void log(const char* message) {
        printf("OK: %s\n", message);
    }

    virtual int add_one(uint32_t x){
        return x + 1;
    }

    virtual void sus(int * in) {
        printf("val: %d\n", *in);
    }
};

class SusLog : public ILog {
    // bool am_chaos = true;
public:
    virtual void log(const char* message) {
        super_important = -1;
        // am_chaos = false;
        printf("OK: %s\n", message);
    }

    virtual int add_one(uint32_t x){
        return x+1;
    }

    virtual void sus(int * in) {
        *in = 99;
    }
};

// Takes an unsigned integer, returns it plus 1.
uint32_t add_one(uint32_t x) {
    ILog* logger;

    int seven = 7;

    if (rand() % 2 == 0) {
        logger = new OkLog();
    } else {
        logger = new SusLog();
    }

    // Concrete class — SAW should execute real code, not an override
    // Validator v(100);
    // Status s = v.check(x);

    logger->log("Adding one to x");

    // If a virtual method clobbered super_important, bail out
    if (super_important == -1) {
        return 12;
    }

    // Use the enum — function should return based on validation status
    // if (s == Status::Error) {
    //     return 0;
    // }

    return x + 1;
}