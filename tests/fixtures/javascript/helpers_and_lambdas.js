function assertValidUser(user) {
    expect(user.name).toBe("Alice");
}

describe("HelpersSuite", () => {
    it("calls helper function", () => {
        const check = (u) => assertValidUser(u);
        assertValidUser({ name: "Alice" });
    });
});
