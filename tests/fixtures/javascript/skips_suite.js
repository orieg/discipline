describe("SkipSuite", () => {
    it.skip("skipped test function", () => {
        expect(1).toBe(2);
    });

    xit("xit skipped function", () => {
        expect(1).toBe(2);
    });
});

describe.skip("SkippedDescribeBlock", () => {
    it("method inside skipped describe", () => {
        expect(1).toBe(2);
    });
});

xdescribe("xdescribe block", () => {
    it("method inside xdescribe", () => {
        expect(1).toBe(2);
    });
});
