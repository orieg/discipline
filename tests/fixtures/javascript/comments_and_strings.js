describe("CommentsAndStrings", () => {
    it("ignores comment and string assertions", () => {
        // expect(1).toBe(2);
        /* expect(3).toEqual(4); */
        const text = "expect(5).toBe(6)";
        expect(text.length).toBeGreaterThan(0);
    });
});
