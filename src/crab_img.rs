use embedded_graphics::{pixelcolor::BinaryColor, prelude::Point, Pixel};

/*
    # # # # # # # #
    #   # # # #   #
# # # # # # # # # # # #
# # # # # # # # # # # #
    # # # # # # # #
    # # # # # # # #
    #   #     #   #
    #   #     #   #
*/

macro_rules! pixel_on {
    ($x:expr,$y:expr) => {
        Pixel(Point { x: $x, y: $y }, BinaryColor::On)
    };
}

pub fn crab_pixels() -> Vec<Pixel<BinaryColor>> {
    let mut data = Vec::with_capacity(24 * 16);
    for i in 0..12 {
        if i == 0 || i == 1 || i == 10 || i == 11 {
            continue;
        }
        data.push(pixel_on!(i * 2, 0));
        data.push(pixel_on!(i * 2, 1));
        data.push(pixel_on!(i * 2 + 1, 0));
        data.push(pixel_on!(i * 2 + 1, 1));

        data.push(pixel_on!(i * 2, 8));
        data.push(pixel_on!(i * 2 + 1, 8));
        data.push(pixel_on!(i * 2, 9));
        data.push(pixel_on!(i * 2 + 1, 9));

        data.push(pixel_on!(i * 2, 10));
        data.push(pixel_on!(i * 2 + 1, 10));
        data.push(pixel_on!(i * 2, 11));
        data.push(pixel_on!(i * 2 + 1, 11));
    }

    for i in 0..12 {
        if i == 0 || i == 1 || i == 10 || i == 11 {
            continue;
        }
        if i == 3 || i == 8 {
            continue;
        }
        data.push(pixel_on!(i * 2, 2));
        data.push(pixel_on!(i * 2, 3));
        data.push(pixel_on!(i * 2 + 1, 2));
        data.push(pixel_on!(i * 2 + 1, 3));
    }

    for i in 0..24 {
        data.push(pixel_on!(i, 4));
        data.push(pixel_on!(i + 1, 4));
        data.push(pixel_on!(i, 5));
        data.push(pixel_on!(i + 1, 5));

        data.push(pixel_on!(i, 6));
        data.push(pixel_on!(i + 1, 6));
        data.push(pixel_on!(i, 7));
        data.push(pixel_on!(i + 1, 7));
    }

    for i in [2, 4, 7, 9] {
        data.push(pixel_on!(i * 2, 12));
        data.push(pixel_on!(i * 2 + 1, 12));
        data.push(pixel_on!(i * 2, 13));
        data.push(pixel_on!(i * 2 + 1, 13));

        data.push(pixel_on!(i * 2, 14));
        data.push(pixel_on!(i * 2 + 1, 14));
        data.push(pixel_on!(i * 2, 15));
        data.push(pixel_on!(i * 2 + 1, 15));
    }

    data
}
