/*
DEMO: Unsat because the functionally is functionally wrong on a basic level. 
The cryptal spec says it should add 1 and it actually adds 7.
*/

// Spec: add_one(x) = x + 1
unsigned int add_one(unsigned int x) {
    return x + 7;
}
