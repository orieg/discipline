describe("Calculator", () => {
    it("adds numbers correctly", () => {
        const sum = 1 + 2;
        expect(sum).toBe(3);
        expect(sum).toEqual(3);
    });

    test("handles positive values", () => {
        expect(10).toBeGreaterThan(0);
    });
});
